//! Recover a first creation that returns outside the game before any usable roster.
use crate::{protocol::Lobby, transport::Client};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Departure {
    requested: bool,
    requested_at: Option<Instant>,
    started: bool,
    left: bool,
    signed_out: Option<Instant>,
    login_requested: bool,
    returned_to_login: bool,
    full_empty_roster_seen: bool,
    creation_failed: bool,
    protected: bool,
    blocked_by: Option<&'static str>,
    roster: Option<RosterFacts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RosterFacts {
    host: bool,
    slots: usize,
    occupied: usize,
    own: usize,
    computer: usize,
    name: bool,
    map: bool,
    empty_slot_placeholder: bool,
    full_empty_roster: bool,
}

impl RosterFacts {
    fn from_lobby(lobby: &Lobby) -> Self {
        Self {
            host: lobby.is_host,
            slots: lobby.players.len(),
            occupied: lobby.players.iter().filter(|p| p.slot_status == 2).count(),
            own: lobby.players.iter().filter(|p| p.is_self).count(),
            computer: lobby.players.iter().filter(|p| p.slot_type == 1).count(),
            name: !lobby.lobby_name.is_empty(),
            map: !lobby.map_data.map_path.is_empty(),
            empty_slot_placeholder: matches!(lobby.players.as_slice(), [p] if p.slot == 0
                && p.slot_status == 0 && p.slot_type == 0 && p.team == u8::MAX
                && !p.is_self && !p.is_observer && p.name.is_empty()),
            // The native client also publishes all 24 empty slots before creation
            // completes. Recovery still requires a failed result or native
            // departure followed by explicit sign-out and a fresh login screen.
            full_empty_roster: lobby.players.len() == 24
                && lobby.players.iter().enumerate().all(|(slot, p)| {
                    usize::from(p.slot) == slot
                        && matches!(p.slot_status, 0 | 1)
                        && p.slot_type == 0
                        && !p.is_self
                        && p.name.is_empty()
                }),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Diagnostic {
    pub left: bool,
    pub login_requested: bool,
    pub returned_to_login: bool,
    pub eligible: bool,
    pub reason: Option<&'static str>,
    pub roster: Option<RosterFacts>,
}

impl Departure {
    pub fn observe_pending(&mut self, event: &mut super::pending::Callback) {
        if !event.departure_observed {
            self.observe(&event.value);
            event.departure_observed = true;
        }
    }

    pub fn requested(&mut self) {
        // Only the first request on a newly owned process can take this path.
        if self.requested {
            self.protect("a repeated creation request");
        }
        self.requested = true;
        self.requested_at.get_or_insert_with(Instant::now);
    }

    pub fn observe(&mut self, event: &Value) {
        match event["messageType"].as_str() {
            Some("MultiplayerGameCreateStarted") if self.started => {
                self.protect("a repeated native creation");
            }
            Some("MultiplayerGameCreateStarted") if self.requested => self.started = true,
            Some("MultiplayerGameCreateStarted") => self.protect("an unsolicited creation"),
            Some("MultiplayerGameLeave") if self.started => self.left = true,
            Some("MultiplayerGameLeave") => self.protect("a departure before creation started"),
            Some("LoggedOut") if self.started => {
                self.signed_out.get_or_insert_with(Instant::now);
            }
            Some("UpdateUserInfo") if event["payload"]["user"]["isAuthenticated"] == true => {
                self.authenticated();
            }
            Some("OnNetProviderInitialized") if event["payload"]["message"]["isOnline"] == true => {
                self.authenticated();
            }
            Some("MultiplayerGameCreateResult") => {
                let result = &event["payload"]["details"]["success"];
                if (*result == false || *result == 0) && self.requested && self.started {
                    self.creation_failed = true;
                } else {
                    self.protect("a successful, unsolicited or unrecognized creation result");
                }
            }
            Some("GameLobbySetup") => {
                let parsed = serde_json::from_value::<Lobby>(event["payload"].clone());
                if self.blocked_by.is_none()
                    && let Ok(lobby) = &parsed
                {
                    self.roster = Some(RosterFacts::from_lobby(lobby));
                }
                if self.started
                    && parsed
                        .as_ref()
                        .is_ok_and(|lobby| RosterFacts::from_lobby(lobby).full_empty_roster)
                {
                    self.full_empty_roster_seen = true;
                }
                let reason = match parsed {
                    Err(_) => Some("an unrecognized roster"),
                    // Warcraft emits this placeholder with either host flag.
                    // It contains no participants and proves neither success
                    // nor departure. A later native departure is still required;
                    // any usable roster or game activity revokes that claim.
                    Ok(ref lobby)
                        if self.started
                            && (RosterFacts::from_lobby(lobby).empty_slot_placeholder
                                || RosterFacts::from_lobby(lobby).full_empty_roster) =>
                    {
                        None
                    }
                    Ok(lobby) if lobby.is_host => Some("a host roster"),
                    Ok(lobby) if !lobby.players.is_empty() => Some("a player roster"),
                    Ok(lobby)
                        if !lobby.lobby_name.is_empty() || !lobby.map_data.map_path.is_empty() =>
                    {
                        Some("lobby metadata")
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    self.protect(reason);
                }
            }
            Some("SetGlueScreen") => {
                // A login screen preceding sign-out is not fresh departure proof.
                if self.signed_out.is_some()
                    && matches!(
                        event["payload"]["screen"].as_str(),
                        Some("LOGIN" | "LOGIN_DOORS")
                    )
                {
                    self.returned_to_login = true;
                }
                let protect = match event["payload"]["screen"].as_str() {
                    Some(
                        "LOGIN"
                        | "LOGIN_DOORS"
                        | "MAIN_MENU"
                        | "CUSTOM_LOBBIES"
                        | "MULTIPLAYER_CUSTOM_GAMES_SETUP",
                    ) => false,
                    Some("GAME_LOBBY") => !self.requested,
                    _ => true,
                };
                if protect {
                    self.protect("a game screen or unrecognized screen");
                }
            }
            Some("LobbyCountdown") => self.protect("a countdown callback"),
            Some("MultiplayerGameJoinStarted" | "MultiplayerGameJoinResult") => {
                self.protect("a game join");
            }
            _ => {}
        }
    }

    pub fn eligible(&self) -> bool {
        self.requested
            && self.started
            && (self.left || (self.signed_out.is_some() && self.returned_to_login))
            && (!self.full_empty_roster_seen
                || self.creation_failed
                || (self.left && self.signed_out.is_some() && self.returned_to_login))
            && !self.protected
    }

    fn authenticated(&mut self) {
        self.signed_out = None;
        self.login_requested = false;
        self.returned_to_login = false;
    }

    fn sign_in_due(&self, now: Instant, budget: Duration) -> bool {
        self.requested
            && self.started
            && !self.protected
            && !self.returned_to_login
            && !self.login_requested
            && (!self.full_empty_roster_seen || self.creation_failed || self.left)
            && (self.left
                || self.requested_at
                    .is_some_and(|at| now.saturating_duration_since(at) >= budget))
            && self
                .signed_out
                .is_some_and(|at| now.saturating_duration_since(at) >= Duration::from_secs(10))
    }

    /// Re-enter native sign-in once after an abandoned, explicitly signed-out
    /// first creation. The request itself never authorizes process termination.
    pub fn request_sign_in(
        &mut self,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        stop: &AtomicBool,
        budget: Duration,
    ) -> Result<(), String> {
        if !self.sign_in_due(Instant::now(), budget) || stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        for event in pending.iter_mut() {
            self.observe_pending(event);
        }
        if !self.sign_in_due(Instant::now(), budget) {
            return Ok(());
        }
        for command in ["SendGameLobbySetup", "GetAuthInfo"] {
            client
                .send(command, serde_json::json!({}))
                .map_err(|e| e.to_string())?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            if let Some(event) = client.receive().map_err(|e| e.to_string())? {
                let mut event = super::pending::Callback::from(event);
                self.observe_pending(&mut event);
                pending.push_back(event);
                quiet_since = Instant::now();
                if !self.sign_in_due(Instant::now(), budget) {
                    return Ok(());
                }
            }
        }
        if !stop.load(Ordering::SeqCst)
            && self.sign_in_due(Instant::now(), budget)
            && quiet_since.elapsed() >= Duration::from_millis(500)
        {
            client
                .send("StartLogin", serde_json::json!({}))
                .map_err(|e| e.to_string())?;
            self.login_requested = true;
        }
        Ok(())
    }

    fn protect(&mut self, reason: &'static str) {
        self.protected = true;
        self.blocked_by.get_or_insert(reason);
    }

    pub fn diagnostic(&self) -> Option<Diagnostic> {
        (self.started
            && (self.left || self.protected || self.login_requested || self.returned_to_login))
            .then_some(Diagnostic {
                left: self.left,
                login_requested: self.login_requested,
                returned_to_login: self.returned_to_login,
                eligible: self.eligible(),
                reason: self.blocked_by,
                roster: self.roster,
            })
    }

    pub fn confirm(
        &mut self,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        stop: &AtomicBool,
    ) -> Result<bool, String> {
        for event in pending.iter_mut() {
            self.observe_pending(event);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet_since = Instant::now();
        while self.eligible() && !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
            if let Some(event) = client.receive().map_err(|e| e.to_string())? {
                let mut event = super::pending::Callback::from(event);
                self.observe_pending(&mut event);
                pending.push_back(event);
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= Duration::from_millis(500) {
                return Ok(self.eligible() && !stop.load(Ordering::SeqCst));
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
#[path = "creation_sign_in_tests.rs"]
mod sign_in_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn left() -> Departure {
        let mut departure = Departure::default();
        departure.requested();
        departure.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        departure.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        departure
    }

    #[test]
    fn failed_first_creation_can_recover_after_explicit_sign_out_and_return_to_login() {
        let mut departure = Departure::default();
        departure.requested();
        departure.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        departure.observe(&json!({"messageType":"LoggedOut"}));
        assert!(
            !departure.eligible(),
            "Sign-out alone is not departure proof"
        );
        departure
            .observe(&json!({"messageType":"SetGlueScreen","payload":{"screen":"LOGIN_DOORS"}}));
        assert!(
            departure.eligible(),
            "The abandoned first creation returned to native sign-in"
        );
    }

    fn placeholder() -> Value {
        json!({"messageType":"GameLobbySetup","payload":{
            "isHost":false,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,
                "isSelf":false,"isObserver":false,"teamChangeEnabled":false,"name":""}]}})
    }

    fn empty_full_roster() -> Value {
        json!({"messageType":"GameLobbySetup","payload":{
            "isHost":true,"lobbyName":"Test #205","mapData":{"mapPath":"map.w3x"},
            "players":(0..24).map(|slot| json!({"slot":slot,"slotStatus":0,
                "slotType":0,"team":slot,"isSelf":false,"isObserver":false,
                "teamChangeEnabled":false,"name":""})).collect::<Vec<_>>()}})
    }

    #[test]
    fn failed_empty_full_roster_then_sign_out_can_recover() {
        let mut state = Departure::default();
        state.requested();
        state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        state.observe(&empty_full_roster());
        state.observe(&json!({"messageType":"LoggedOut"}));
        state.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
        assert!(
            !state.eligible(),
            "A failed result is not fresh departure proof"
        );
        state.observe(&json!({"messageType":"SetGlueScreen","payload":{"screen":"LOGIN_DOORS"}}));
        assert!(
            state.eligible(),
            "An empty 24-slot failed creation must not block recovery forever"
        );
    }

    #[test]
    fn empty_full_roster_requires_failure_and_never_unprotects_live_activity() {
        let mut state = left();
        state.observe(&empty_full_roster());
        assert!(
            !state.eligible(),
            "Empty roster and leave cannot replace the failed result"
        );
        let failure = json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}});
        state.observe(&failure);
        assert!(state.eligible());
        for (pointer, value) in [
            ("/payload/players/0/slot", json!(1)),
            ("/payload/players/0/slotStatus", json!(2)),
            ("/payload/players/0/slotStatus", json!(9)),
            ("/payload/players/0/slotType", json!(1)),
            ("/payload/players/0/isSelf", json!(true)),
            ("/payload/players/0/name", json!("Participant")),
        ] {
            let mut event = empty_full_roster();
            *event.pointer_mut(pointer).unwrap() = value;
            let mut d = left();
            d.observe(&event);
            d.observe(&failure);
            d.observe(&empty_full_roster());
            assert!(!d.eligible(), "{pointer} must remain protected");
        }
        for event in [
            json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":true}}}),
            json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}}),
            json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":true}}}),
            json!({"messageType":"MultiplayerGameJoinStarted"}),
        ] {
            let mut d = left();
            d.observe(&empty_full_roster());
            d.observe(&event);
            d.observe(&failure);
            d.observe(&json!({"messageType":"LoggedOut"}));
            d.observe(&json!({"messageType":"SetGlueScreen","payload":{"screen":"LOGIN_DOORS"}}));
            assert!(!d.eligible(), "{event} must remain protected");
        }
    }

    #[test]
    fn failure_before_the_requested_native_creation_cannot_authorize_recovery() {
        for requested in [false, true] {
            let mut state = Departure::default();
            if requested {
                state.requested();
            }
            state.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
            assert!(!state.creation_failed);
            if !requested {
                state.requested();
            }
            state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
            state.observe(&empty_full_roster());
            state.observe(&json!({"messageType":"MultiplayerGameLeave"}));
            assert!(!state.eligible());
            assert!(!state.sign_in_due(Instant::now(), Duration::ZERO));
        }
    }

    #[test]
    fn a_second_native_creation_revokes_old_failure_and_departure_evidence() {
        for signed_out in [false, true] {
            let mut state = Departure::default();
            state.requested();
            state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
            state.observe(&empty_full_roster());
            state.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
            if signed_out {
                state.observe(&json!({"messageType":"LoggedOut"}));
                state.observe(
                    &json!({"messageType":"SetGlueScreen","payload":{"screen":"LOGIN_DOORS"}}),
                );
            } else {
                state.observe(&json!({"messageType":"MultiplayerGameLeave"}));
            }
            assert!(state.eligible());
            state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
            state.observe(&empty_full_roster());
            assert!(!state.eligible());
            state.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
            state.observe(&json!({"messageType":"MultiplayerGameLeave"}));
            assert!(
                !state.eligible(),
                "Only the first native attempt may recover this way"
            );
            assert!(!state.sign_in_due(Instant::now(), Duration::ZERO));
        }
    }

    #[test]
    fn native_host_flag_on_a_single_empty_placeholder_does_not_block_confirmed_departure() {
        let mut event = placeholder();
        event["payload"]["isHost"] = json!(true);
        let mut state = Departure::default();
        state.requested();
        state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        state.observe(&event);
        assert!(
            !state.eligible(),
            "The placeholder itself is not departure proof"
        );
        state.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        assert!(
            state.eligible(),
            "Native departure closed the unpopulated first creation"
        );
    }

    #[test]
    fn an_empty_creation_placeholder_needs_a_native_departure_before_recovery() {
        let mut d = Departure::default();
        d.requested();
        d.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        d.observe(&placeholder());
        assert!(!d.eligible());
        d.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        assert!(
            d.eligible(),
            "The native placeholder contains no participants"
        );
        d.observe(&placeholder());
        assert!(d.eligible());
    }

    #[test]
    fn placeholder_exemption_does_not_cover_unsolicited_or_usable_rosters() {
        let mut unsolicited = Departure::default();
        unsolicited.requested();
        unsolicited.observe(&placeholder());
        unsolicited.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        unsolicited.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        assert!(!unsolicited.eligible());
        for (pointer, value) in [
            ("/payload/players/0/slot", json!(1)),
            ("/payload/players/0/slotStatus", json!(2)),
            ("/payload/players/0/slotType", json!(1)),
            ("/payload/players/0/team", json!(0)),
            ("/payload/players/0/isSelf", json!(true)),
            ("/payload/players/0/isObserver", json!(true)),
            ("/payload/players/0/name", json!("Participant")),
        ] {
            let mut event = placeholder();
            *event.pointer_mut(pointer).unwrap() = value;
            let mut d = left();
            d.observe(&placeholder());
            d.observe(&event);
            d.observe(&placeholder());
            assert!(!d.eligible(), "{pointer}");
        }
        let mut multiple = placeholder();
        let row = multiple["payload"]["players"][0].clone();
        multiple["payload"]["players"]
            .as_array_mut()
            .unwrap()
            .push(row);
        let mut d = left();
        d.observe(&multiple);
        assert!(!d.eligible());
    }

    #[test]
    fn only_a_requested_started_creation_and_native_leave_can_authorize_recovery() {
        let mut d = Departure::default();
        assert!(!d.eligible());
        d.requested();
        assert!(!d.eligible());
        d.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        assert!(!d.eligible());
        d.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
        assert!(!d.eligible());
        d.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        assert!(d.eligible());
        d.requested();
        assert!(
            !d.eligible(),
            "a second request cannot reuse departure evidence"
        );
        let mut unsolicited = Departure::default();
        unsolicited.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        unsolicited.requested();
        unsolicited.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        unsolicited.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        assert!(!unsolicited.eligible());
    }

    #[test]
    fn roster_loading_success_join_or_unknown_state_permanently_protects_the_game() {
        let mut protected_events = vec![
            json!({"messageType":"GameLobbySetup","payload":{}}),
            json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":true}}}),
            json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":"unknown"}}}),
            json!({"messageType":"MultiplayerGameJoinStarted"}),
            json!({"messageType":"MultiplayerGameJoinResult"}),
            json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":false}}}),
        ];
        for screen in ["LOADING_SCREEN", "SCORE_SCREEN", "CAMPAIGN"] {
            protected_events
                .push(json!({"messageType":"SetGlueScreen","payload":{"screen":screen}}));
        }
        for own in [false, true] {
            protected_events.push(json!({"messageType":"GameLobbySetup","payload":{
                "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
                "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,
                    "isSelf":own,"isObserver":true,"teamChangeEnabled":true}]}}));
        }
        for event in protected_events {
            let mut d = left();
            d.observe(&event);
            d.observe(&json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}));
            d.observe(&json!({"messageType":"MultiplayerGameLeave"}));
            assert!(!d.eligible(), "{event}");
        }
    }
}

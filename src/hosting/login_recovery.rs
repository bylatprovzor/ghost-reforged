//! Retained startup ownership for sign-out or departure before any usable lobby.
use crate::{background_startup::Startup, protocol::Lobby, transport::Client};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Recovery {
    owner: Startup,
    evidence: Evidence,
    checked: Option<Instant>,
    departure: super::creation_departure::Departure,
    reported_departure: Option<(bool, super::creation_departure::Diagnostic)>,
    session: super::session_recovery::Session,
}

impl Recovery {
    pub fn observe_pending(&mut self, event: &mut super::pending::Callback, now: Instant) {
        self.evidence.observe(&event.value, now);
        self.departure.observe_pending(event);
        self.session.observe_pending(event, now);
    }

    /// Transfer the retained process claim before listener readiness revokes it.
    pub fn listener_ready(&mut self, startup: &mut Startup, pid: u32) {
        if startup.owns(pid) {
            self.owner = std::mem::take(startup);
            self.evidence = Evidence::default();
            self.checked = None;
            self.departure = Default::default();
            self.reported_departure = None;
            self.session = Default::default();
        }
        startup.ready();
    }

    pub fn observe(&mut self, event: &Value, now: Instant) {
        self.evidence.observe(event, now);
        self.departure.observe(event);
        self.session.observe(event, now);
    }

    pub fn disconnected_idle_lobby(
        &mut self,
        engine: &crate::engine::Engine,
        identity_ok: bool,
        now: u64,
        epoch: Option<u64>,
    ) {
        if engine.phase == crate::engine::Phase::Lobby
            && identity_ok
            && engine.owned_lobby(now).is_some_and(Lobby::empty)
        {
            self.session.disconnected_idle_lobby(epoch);
        }
    }

    pub fn awaiting_session_reply(&self, pid: u32) -> bool {
        self.owner.owns(pid) && self.session.awaiting_reply(Instant::now())
    }

    pub fn connection_changed(&mut self) {
        self.session.connection_changed();
    }

    pub fn creation_requested(&mut self) {
        self.evidence.protected = true;
        self.departure.requested();
    }

    pub fn diagnostic(&mut self, pid: u32) -> Option<String> {
        if let Some(notice) = self.session.take_notice() {
            return Some(notice.into());
        }
        let facts = self.departure.diagnostic()?;
        let state = (self.owner.owns(pid), facts);
        if self.reported_departure == Some(state) {
            return None;
        }
        self.reported_departure = Some(state);
        let mut message = format!(
            "Creation recovery guard: owned={}, departed={}, login_requested={}, returned_to_login={}, eligible={}, blocked_by={}",
            state.0,
            facts.left,
            facts.login_requested,
            facts.returned_to_login,
            facts.eligible,
            facts.reason.unwrap_or("none")
        );
        if let Some(roster) = facts.roster {
            message.push_str(&format!(", roster={roster:?}"));
        }
        Some(message)
    }

    pub fn due(&self, pid: u32, now: Instant, budget: Duration) -> bool {
        self.owner.owns(pid)
            && (self.evidence.expired(now, budget)
                || self.departure.eligible()
                || self.session.due(now, budget))
            && self
                .checked
                .is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(30))
    }

    pub fn confirm(
        &mut self,
        pid: u32,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        stop: &AtomicBool,
        budget: Duration,
    ) -> Result<bool, String> {
        if self.owner.owns(pid) && !stop.load(Ordering::SeqCst) {
            self.departure
                .request_sign_in(client, pending, stop, budget)?;
        }
        if !self.due(pid, Instant::now(), budget) || stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.checked = Some(Instant::now());
        let confirmed = if self.session.due(Instant::now(), budget) {
            self.session.confirm(client, pending, stop, budget)?
        } else if self.departure.eligible() {
            self.departure.confirm(client, pending, stop)?
        } else {
            self.evidence.confirm(client, pending, stop, budget)?
        };
        Ok(confirmed && self.owner.owns(pid) && !stop.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
struct Evidence {
    signed_out: Option<Instant>,
    pending_auth: Option<Instant>,
    first_login_screen: Option<Instant>,
    idle_menu: bool,
    login_menu: bool,
    protected: bool,
}

impl Evidence {
    fn observe(&mut self, event: &Value, now: Instant) {
        match event["messageType"].as_str() {
            Some("LoggedOut") => {
                self.signed_out.get_or_insert(now);
            }
            Some("UpdateUserInfo") if authenticated(event) == Some(true) => {
                self.signed_out = None;
                self.pending_auth = None;
                self.first_login_screen = None;
            }
            Some("UpdateUserInfo") if authenticated(event) == Some(false) => {
                self.pending_auth.get_or_insert(now);
            }
            Some("OnNetProviderInitialized") if event["payload"]["message"]["isOnline"] == true => {
                self.signed_out = None;
                self.pending_auth = None;
                self.first_login_screen = None;
            }
            Some("SetGlueScreen") => {
                let screen = event["payload"]["screen"].as_str();
                self.idle_menu = matches!(screen, Some("LOGIN" | "LOGIN_DOORS" | "MAIN_MENU"));
                self.login_menu = matches!(screen, Some("LOGIN" | "LOGIN_DOORS"));
                if self.login_menu {
                    self.first_login_screen.get_or_insert(now);
                }
                self.protected |= !self.idle_menu;
            }
            Some("GameLobbySetup") => {
                // Unknown or populated snapshots cannot authorize a startup reset.
                self.protected |= serde_json::from_value::<Lobby>(event["payload"].clone()).map_or(
                    true,
                    |lobby| {
                        lobby.is_host
                            || !lobby.players.is_empty()
                            || !lobby.lobby_name.is_empty()
                            || !lobby.map_data.map_path.is_empty()
                    },
                );
            }
            Some(
                "MultiplayerGameCreateStarted"
                | "MultiplayerGameCreateResult"
                | "MultiplayerGameJoinStarted"
                | "MultiplayerGameJoinResult"
                | "MultiplayerGameLeave"
                | "LobbyCountdown",
            ) => self.protected = true,
            _ => {}
        }
    }

    fn expired(&self, now: Instant, budget: Duration) -> bool {
        let elapsed =
            |at: Option<Instant>| at.is_some_and(|at| now.saturating_duration_since(at) >= budget);
        // A cold client can report negative authentication long before its login
        // UI is ready. Give that first transition a bounded sign-in interval;
        // repeated screen/auth callbacks must not extend it.
        let login_wait = self
            .pending_auth
            .zip(self.first_login_screen)
            .map(|(pending, screen)| pending.max(screen));
        !self.protected
            && self.idle_menu
            && (elapsed(self.signed_out) || (self.login_menu && elapsed(login_wait)))
    }

    /// Only an explicitly unauthenticated, never-used startup can reach this check.
    /// GetAuthInfo is not guaranteed to repeat UpdateUserInfo after native logout.
    /// Drain callbacks and allow late login/lobby activity to revoke the claim.
    fn confirm(
        &mut self,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        stop: &AtomicBool,
        budget: Duration,
    ) -> Result<bool, String> {
        for event in pending.iter() {
            self.observe(event, Instant::now());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            let Some(event) = client.receive_available().map_err(|e| e.to_string())? else {
                break;
            };
            self.observe(&event, Instant::now());
            pending.push_back(event.into());
        }
        if !self.expired(Instant::now(), budget)
            || Instant::now() >= deadline
            || stop.load(Ordering::SeqCst)
        {
            return Ok(false);
        }
        client
            .send("SendGameLobbySetup", json!({}))
            .map_err(|e| e.to_string())?;
        client
            .send("GetAuthInfo", json!({}))
            .map_err(|e| e.to_string())?;
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            if let Some(event) = client.receive().map_err(|e| e.to_string())? {
                quiet_since = Instant::now();
                self.observe(&event, Instant::now());
                pending.push_back(event.into());
                if !self.expired(Instant::now(), budget) {
                    return Ok(false);
                }
            }
        }
        Ok(!stop.load(Ordering::SeqCst)
            && self.expired(Instant::now(), budget)
            && quiet_since.elapsed() >= Duration::from_millis(500))
    }
}

fn authenticated(event: &Value) -> Option<bool> {
    (event["messageType"] == "UpdateUserInfo")
        .then(|| event["payload"]["user"]["isAuthenticated"].as_bool())
        .flatten()
}

#[cfg(test)]
#[path = "login_recovery_tests.rs"]
mod tests;

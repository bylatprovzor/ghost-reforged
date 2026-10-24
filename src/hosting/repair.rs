use super::{Event, status};
use crate::{
    engine::Phase,
    protocol::{self, Lobby},
    recovery::incomplete_lobby,
    transport::Client,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Repair {
    attempted: bool,
    protected: bool,
    candidate: Option<(String, String, u64)>,
    creating_until: Option<Instant>,
    creation: Option<super::creation_recovery::Request>,
    native_creation_started: bool,
}

impl Repair {
    pub fn creation_sent(&mut self, request: super::creation_recovery::Request) {
        self.creation = Some(request);
        self.native_creation_started = false;
    }

    pub fn forget_creation(&mut self) {
        self.creation = None;
        self.native_creation_started = false;
    }

    pub fn inspect_pending(&mut self, event: &mut super::pending::Callback) {
        if event.repair_observed {
            return;
        }
        if event["messageType"] == "MultiplayerGameCreateStarted" {
            if self.native_creation_started || self.creation.is_none() {
                self.protected = true;
            }
            self.native_creation_started = true;
        }
        self.inspect(event);
        event.repair_observed = true;
    }

    pub fn inspect(&mut self, event: &Value) {
        self.observe_location(event);
        if self.protected {
            self.creation = None;
        } else if let Some(request) = &mut self.creation {
            request.observe(event);
        }
    }

    pub fn rejected_lobby(&self, lobby: &Lobby) -> bool {
        !self.protected
            && self
                .creation
                .as_ref()
                .is_some_and(|request| request.matches(lobby))
    }

    pub fn repairable(&self, lobby: &Lobby) -> bool {
        incomplete_lobby(lobby) || self.rejected_lobby(lobby)
    }

    pub fn creation_started(&mut self, budget: Duration) {
        self.creating_until = Some(Instant::now() + budget);
        self.candidate = None;
    }

    fn waiting_for_creation(&self) -> bool {
        self.creating_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Evaluate an event already inspected at receipt; replay must not rewind
    /// the newer safety evidence retained across a handshake or queue.
    pub fn observe_dispatched(
        &mut self,
        phase: &Phase,
        event: Option<&Value>,
        now: u64,
    ) -> Option<Lobby> {
        if let Some(event) = event {
            if event["messageType"] == "GameLobbySetup"
                && serde_json::from_value::<Lobby>(event["payload"].clone())
                    .is_ok_and(|lobby| !incomplete_lobby(&lobby) && lobby.local().is_some())
            {
                self.creating_until = None;
            }
        }
        if self.protected
            || self.waiting_for_creation()
            || !matches!(
                phase,
                Phase::Lobby | Phase::Countdown(_) | Phase::Recovering(_)
            )
        {
            self.candidate = None;
            return None;
        }
        let event = event?;
        if interrupts(event) {
            self.candidate = None;
        }
        if event["messageType"] != "GameLobbySetup" {
            return None;
        }
        let lobby = serde_json::from_value::<Lobby>(event["payload"].clone()).ok()?;
        if !self.repairable(&lobby) {
            self.candidate = None;
            return None;
        }
        let identity = (lobby.lobby_name.clone(), lobby.map_data.map_path.clone());
        let since = match &self.candidate {
            Some((name, map, since)) if name == &identity.0 && map == &identity.1 => *since,
            _ => {
                self.candidate = Some((identity.0, identity.1, now));
                now
            }
        };
        (now.saturating_sub(since) >= 3).then_some(lobby)
    }

    fn observe_location(&mut self, event: &Value) {
        if event["messageType"] == "SetGlueScreen" {
            match event["payload"]["screen"].as_str() {
                Some("LOADING_SCREEN" | "SCORE_SCREEN") => self.protected = true,
                Some("MAIN_MENU" | "MULTIPLAYER_CUSTOM_GAMES_SETUP" | "CUSTOM_LOBBIES") => {
                    self.protected = false;
                }
                _ => {}
            }
        } else if event["messageType"] == "LobbyCountdown"
            && event["payload"]["details"]["countdownActive"] == true
        {
            self.protected = true;
        }
    }

    /// Retain every callback for normal dispatch if the fresh snapshot cancels repair.
    pub fn confirm_with_recovery(
        &mut self,
        candidate: &Lobby,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        tx: &Sender<Event>,
        stop: &AtomicBool,
        recovery: &mut super::login_recovery::Recovery,
    ) -> Result<bool, String> {
        let result = self.confirm(candidate, client, pending, tx, stop);
        // A transport error must not discard evidence already received from the
        // same game process. Observe before the caller propagates the error.
        for event in pending.iter_mut() {
            recovery.observe_pending(event, Instant::now());
        }
        result
    }

    pub fn confirm(
        &mut self,
        candidate: &Lobby,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        tx: &Sender<Event>,
        stop: &AtomicBool,
    ) -> Result<bool, String> {
        if !self.repairable(candidate)
            || (self.waiting_for_creation() && !self.rejected_lobby(candidate))
            || stop.load(Ordering::SeqCst)
        {
            return Ok(false);
        }
        // Startup can contain an older menu callback followed by the current roster.
        // A roster supersedes that menu, but never erases evidence of loading.
        let mut compatible = true;
        for event in pending.iter_mut() {
            let newly_received = !event.repair_observed;
            self.inspect_pending(event);
            if event["messageType"] == "MultiplayerGameCreateStarted" {
                if newly_received {
                    self.protected = true;
                    return Ok(false);
                }
                // The expected start was already inspected before confirmation.
                // A second start has already set protection in inspect_pending.
                continue;
            }
            if event["messageType"] == "GameLobbySetup" || interrupts(event) {
                compatible = !self.contradicts(candidate, event);
            }
        }
        if self.protected || !compatible {
            return Ok(false);
        }
        status(tx, "Checking incomplete lobby…");
        let deadline = Instant::now() + Duration::from_secs(3);
        // Drain queued callbacks before asking for a snapshot that can authorize restart.
        while Instant::now() < deadline {
            match client.receive_available().map_err(|e| e.to_string())? {
                Some(event) => {
                    let mut event = super::pending::Callback::from(event);
                    self.inspect_pending(&mut event);
                    if event["messageType"] == "MultiplayerGameCreateStarted" {
                        self.protected = true;
                    }
                    let cancel = self.contradicts(candidate, &event);
                    pending.push_back(event);
                    if cancel || stop.load(Ordering::SeqCst) {
                        return Ok(false);
                    }
                }
                None => break,
            }
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        client
            .send("SendGameLobbySetup", json!({}))
            .map_err(|e| e.to_string())?;
        let mut fresh = false;
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            if let Some(event) = client.receive().map_err(|e| e.to_string())? {
                let mut event = super::pending::Callback::from(event);
                self.inspect_pending(&mut event);
                if event["messageType"] == "MultiplayerGameCreateStarted" {
                    self.protected = true;
                }
                let cancel = self.contradicts(candidate, &event);
                fresh |= event["messageType"] == "GameLobbySetup";
                pending.push_back(event);
                if cancel {
                    return Ok(false);
                }
            } else if fresh {
                if self.attempted {
                    return Err("Warcraft III still returned an incomplete lobby after restarting. Check the selected map before hosting again.".into());
                }
                self.attempted = true;
                self.candidate = None;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn contradicts(&self, candidate: &Lobby, event: &Value) -> bool {
        if interrupts(event) {
            return true;
        }
        if event["messageType"] != "GameLobbySetup" {
            return false;
        }
        serde_json::from_value::<Lobby>(event["payload"].clone()).map_or(true, |lobby| {
            !self.repairable(&lobby)
                || candidate.lobby_name != lobby.lobby_name
                || !protocol::same_map_path(&candidate.map_data.map_path, &lobby.map_data.map_path)
        })
    }
}

fn interrupts(event: &Value) -> bool {
    event["messageType"] == "MultiplayerGameCreateStarted"
        || event["messageType"] == "MultiplayerGameLeave"
        || event["messageType"] == "LoggedOut"
        || (event["messageType"] == "LobbyCountdown"
            && event["payload"]["details"]["countdownActive"] == true)
        || (event["messageType"] == "SetGlueScreen" && event["payload"]["screen"] != "GAME_LOBBY")
}

#[cfg(test)]
#[path = "repair_tests.rs"]
mod tests;

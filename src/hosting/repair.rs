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
}

impl Repair {
    pub fn observe(&mut self, phase: &Phase, event: Option<&Value>, now: u64) -> Option<Lobby> {
        if let Some(event) = event {
            self.observe_location(event);
        }
        if self.protected
            || !matches!(
                phase,
                Phase::Creating(_) | Phase::Lobby | Phase::Countdown(_)
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
        if !incomplete_lobby(&lobby) {
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
    pub fn confirm(
        &mut self,
        candidate: &Lobby,
        client: &mut Client,
        pending: &mut VecDeque<Value>,
        tx: &Sender<Event>,
        stop: &AtomicBool,
    ) -> Result<bool, String> {
        if !incomplete_lobby(candidate) || stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        // Startup can contain an older menu callback followed by the current roster.
        // A roster supersedes that menu, but never erases evidence of loading.
        let mut compatible = true;
        for event in pending.iter() {
            self.observe_location(event);
            if event["messageType"] == "GameLobbySetup" || interrupts(event) {
                compatible = !contradicts(candidate, event);
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
                    let cancel = contradicts(candidate, &event);
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
                let cancel = contradicts(candidate, &event);
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
}

fn interrupts(event: &Value) -> bool {
    event["messageType"] == "MultiplayerGameLeave"
        || event["messageType"] == "LoggedOut"
        || (event["messageType"] == "LobbyCountdown"
            && event["payload"]["details"]["countdownActive"] == true)
        || (event["messageType"] == "SetGlueScreen" && event["payload"]["screen"] != "GAME_LOBBY")
}

fn contradicts(candidate: &Lobby, event: &Value) -> bool {
    if interrupts(event) {
        return true;
    }
    if event["messageType"] != "GameLobbySetup" {
        return false;
    }
    serde_json::from_value::<Lobby>(event["payload"].clone()).map_or(true, |lobby| {
        !incomplete_lobby(&lobby)
            || candidate.lobby_name != lobby.lobby_name
            || !protocol::same_map_path(&candidate.map_data.map_path, &lobby.map_data.map_path)
    })
}

#[cfg(test)]
#[path = "repair_tests.rs"]
mod tests;

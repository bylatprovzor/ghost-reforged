//! Recover explicit sign-out from an idle, owned lobby without touching a game.
use crate::{protocol::Lobby, transport::Client};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Session {
    signed_out: Option<Instant>,
    idle_lobby: bool,
    requested: Option<Instant>,
    login_screen: bool,
    blocked: bool,
    notice: bool,
    epoch: u64,
}

impl Session {
    pub fn disconnected_idle_lobby(&mut self, epoch: Option<u64>) {
        if epoch == Some(self.epoch) && self.signed_out.is_some() && !self.blocked {
            self.idle_lobby = true;
        }
    }

    pub fn observe(&mut self, event: &Value, now: Instant) {
        match event["messageType"].as_str() {
            Some("LoggedOut") => {
                self.signed_out.get_or_insert(now);
            }
            Some("UpdateUserInfo") if event["payload"]["user"]["isAuthenticated"] == true => {
                self.authenticated();
            }
            Some("OnNetProviderInitialized") if event["payload"]["message"]["isOnline"] == true => {
                self.authenticated();
            }
            _ if self.signed_out.is_none() => {}
            Some("SetGlueScreen") => {
                self.login_screen = matches!(
                    event["payload"]["screen"].as_str(),
                    Some("LOGIN" | "LOGIN_DOORS")
                );
                if !self.login_screen {
                    self.blocked = true;
                }
            }
            Some("GameLobbySetup") => {
                // A silent getter is common after logout. A replying occupied or
                // malformed roster, however, always vetoes recovery.
                self.blocked |= serde_json::from_value::<Lobby>(event["payload"].clone()).map_or(
                    true,
                    |lobby| {
                        lobby.players.iter().any(|player| {
                            !matches!(player.slot_status, 0 | 1)
                                || player.slot_type != 0
                                || player.is_self
                                || !player.name.is_empty()
                        })
                    },
                );
            }
            Some(
                "MultiplayerGameCreateStarted"
                | "MultiplayerGameCreateResult"
                | "MultiplayerGameJoinStarted"
                | "MultiplayerGameJoinResult"
                | "LobbyCountdown",
            ) => self.blocked = true,
            _ => {}
        }
    }

    pub fn due(&self, now: Instant, budget: Duration) -> bool {
        self.idle_lobby
            && !self.blocked
            && self
                .signed_out
                .is_some_and(|at| now.saturating_duration_since(at) >= budget)
            && self.requested.is_none_or(|at| {
                self.login_screen && now.saturating_duration_since(at) >= Duration::from_secs(30)
            })
    }

    fn authenticated(&mut self) {
        *self = Self {
            epoch: self.epoch.wrapping_add(1),
            ..Self::default()
        };
    }

    pub fn awaiting_reply(&self, now: Instant) -> bool {
        self.signed_out.is_some()
            && !self.blocked
            && self
                .requested
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(60))
    }

    pub fn connection_changed(&mut self) {
        // A transition sent to the lost socket may never be replayed. Retain the
        // logout/ownership budget, but request fresh sign-in on the new socket.
        self.requested = None;
        self.login_screen = false;
    }

    /// Call only with a retained claim to this exact game process. A native
    /// sign-in request is not termination proof: require its return to login,
    /// allow authentication time, and recheck for late activity before restart.
    pub fn confirm(
        &mut self,
        client: &mut Client,
        pending: &mut VecDeque<super::pending::Callback>,
        stop: &AtomicBool,
        budget: Duration,
    ) -> Result<bool, String> {
        for event in pending.iter_mut() {
            self.observe_pending(event, Instant::now());
        }
        let drain_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < drain_deadline && !stop.load(Ordering::SeqCst) {
            let Some(event) = client.receive_available().map_err(|e| e.to_string())? else {
                break;
            };
            self.retain(event, pending);
        }
        if Instant::now() >= drain_deadline
            || stop.load(Ordering::SeqCst)
            || !self.due(Instant::now(), budget)
        {
            return Ok(false);
        }
        for request in ["SendGameLobbySetup", "GetAuthInfo"] {
            client.send(request, json!({})).map_err(|e| e.to_string())?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet_since = Instant::now();
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            if let Some(event) = client.receive().map_err(|e| e.to_string())? {
                self.retain(event, pending);
                quiet_since = Instant::now();
                if !self.due(Instant::now(), budget) {
                    return Ok(false);
                }
            }
        }
        if stop.load(Ordering::SeqCst)
            || !self.due(Instant::now(), budget)
            || quiet_since.elapsed() < Duration::from_millis(500)
        {
            return Ok(false);
        }
        if self.requested.is_none() {
            client
                .send("StartLogin", json!({}))
                .map_err(|e| e.to_string())?;
            self.requested = Some(Instant::now());
            self.notice = true;
            // Only a screen observed after this request can authorize restart.
            self.login_screen = false;
            return Ok(false);
        }
        Ok(self.login_screen)
    }

    pub fn take_notice(&mut self) -> Option<&'static str> {
        std::mem::take(&mut self.notice).then_some(
            "The empty hosted lobby signed out. Requested native Battle.net sign-in before recovery.",
        )
    }

    pub fn observe_pending(&mut self, event: &mut super::pending::Callback, now: Instant) {
        if !event.session_observed {
            self.observe(&event.value, now);
            if event["messageType"] == "LoggedOut" {
                event.session_epoch = Some(self.epoch);
            }
            event.session_observed = true;
        }
    }

    fn retain(&mut self, event: Value, pending: &mut VecDeque<super::pending::Callback>) {
        let mut event = super::pending::Callback::from(event);
        self.observe_pending(&mut event, Instant::now());
        pending.push_back(event);
    }
}

#[cfg(test)]
#[path = "session_recovery_tests.rs"]
mod tests;

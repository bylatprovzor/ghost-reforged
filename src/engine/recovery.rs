use super::*;

impl Engine {
    /// A retry must reconcile native state before issuing another create/start.
    pub(crate) fn recover(&mut self, now: u64) {
        if self.session_replaced {
            return;
        }
        if matches!(
            self.phase,
            Phase::InGame | Phase::WatchingGame | Phase::Restarting | Phase::Releasing(_)
        ) {
            return;
        }
        if self.awaiting_session() || (!self.owned && matches!(self.phase, Phase::Stopped(_))) {
            self.phase = Phase::AwaitMenu;
            self.provider_ready = false;
            // Reopening the socket cannot acknowledge an unfinished menu.
            self.initialization_required = true;
            self.authenticated_initialization = false;
            return;
        }
        if !matches!(self.phase, Phase::Recovering(_)) {
            self.recovery_idle = self.owned
                && self.phase == Phase::Lobby
                && self.start.is_none()
                && !self.countdown_locked
                && !self.start_confirmed;
            self.recovery_cancel = None;
        }
        self.phase = Phase::Recovering(now);
        self.start = None;
        self.move_sent = None;
    }

    pub(crate) fn client_lost(&mut self, now: u64) {
        if self.game_started() {
            self.finish_game(now);
        } else {
            self.retry_incomplete_lobby();
        }
    }

    /// Only callbacks from the reconnected socket reach this path.
    pub(super) fn reconcile(&mut self, event: &Value, now: u64) -> Vec<Command> {
        let kind = event["messageType"].as_str().unwrap_or("");
        let payload = &event["payload"];
        if kind == "LobbyCountdown" && payload["details"]["countdownActive"] == true {
            self.recovery_idle = false;
        }
        if kind == "SetGlueScreen" && payload["screen"] == "LOADING_SCREEN" {
            self.phase = if self.owned {
                Phase::Starting(now)
            } else {
                Phase::WatchingGame
            };
            return self.on_event(event, now);
        }
        if kind == "MultiplayerGameLeave"
            || (kind == "SetGlueScreen"
                && matches!(
                    payload["screen"].as_str(),
                    Some("MAIN_MENU" | "CUSTOM_LOBBIES" | "MULTIPLAYER_CUSTOM_GAMES_SETUP")
                ))
        {
            if self.game_started() {
                self.finish_game(now);
            } else if self.owned {
                self.release(now);
            } else {
                self.retry_incomplete_lobby();
            }
            self.menu_required = false;
            return vec![Command::empty("GetGameList")];
        }
        if kind == "SetGlueScreen" && payload["screen"] == "SCORE_SCREEN" {
            self.phase = if self.owned {
                Phase::InGame
            } else {
                Phase::WatchingGame
            };
            return self.on_event(event, now);
        }
        if kind == "LobbyCountdown"
            && payload["details"]["countdownActive"] == false
            && self.recovery_cancel.is_some()
            && self.latest.is_some()
        {
            self.phase = Phase::Lobby;
            self.countdown_locked = false;
            self.start_confirmed = false;
            self.cancelled_roster = None;
            self.retry_start_after = now.saturating_add(5);
            return vec![Command::empty("SendGameLobbySetup")];
        }
        if kind != "GameLobbySetup" {
            return Vec::new();
        }
        let Ok(lobby) = serde_json::from_value::<Lobby>(payload.clone()) else {
            return Vec::new();
        };
        if !lobby.is_host
            || lobby.lobby_name != self.config.name(self.number)
            || !protocol::same_map_path(
                &lobby.map_data.map_path,
                &protocol::client_path(&self.config.maps[self.map_index]),
            )
            || lobby
                .local()
                .is_none_or(|p| p.slot_status != 2 || p.slot_type != 0)
        {
            return Vec::new();
        }
        self.owned = true;
        let named_host = lobby.local().is_some_and(|player| !player.name.is_empty());
        self.latest = Some((lobby, now));
        if named_host {
            self.phase = Phase::Leaving(now);
            return vec![Command::empty("LobbyCancel"), Command::empty("LeaveGame")];
        }
        if self.recovery_idle {
            // No start was outstanding before the socket failed. A fresh matching
            // roster restores that lobby without a cancel that may never acknowledge.
            self.phase = Phase::Lobby;
            self.retry_start_after = now.saturating_add(5);
            return vec![Command::empty("SendGameLobbySetup")];
        }
        match self.recovery_cancel {
            None => {
                self.recovery_cancel = Some(now);
                vec![Command::empty("LobbyCancel")]
            }
            Some(sent) if now.saturating_sub(sent) >= 10 => {
                // Still in our lobby after cancellation: recreate it if the
                // native acknowledgement never arrives. Never terminate a game.
                self.phase = Phase::Leaving(now);
                vec![Command::empty("LeaveGame")]
            }
            _ => Vec::new(),
        }
    }
}

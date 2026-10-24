//! A delayed observer departure cannot retain authority over a newer session.
use super::*;

impl Engine {
    pub(crate) fn session_replaced(&self) -> bool {
        self.session_replaced
    }

    pub(super) fn departure_replaced(&self, event: &Value) -> bool {
        if !matches!(self.phase, Phase::Releasing(_)) {
            return false;
        }
        match event["messageType"].as_str() {
            Some(
                "MultiplayerGameCreateStarted"
                | "MultiplayerGameJoinStarted"
                | "MultiplayerGameJoinResult",
            ) => true,
            Some("LobbyCountdown") => event["payload"]["details"]["countdownActive"] != false,
            Some("SetGlueScreen") => !matches!(
                event["payload"]["screen"].as_str(),
                Some(
                    "LOADING_SCREEN"
                        | "SCORE_SCREEN"
                        | "MAIN_MENU"
                        | "CUSTOM_LOBBIES"
                        | "MULTIPLAYER_CUSTOM_GAMES_SETUP"
                        | "LOGIN"
                        | "LOGIN_DOORS"
                )
            ),
            Some("GameLobbySetup") => {
                let Ok(lobby) = serde_json::from_value::<Lobby>(event["payload"].clone()) else {
                    return true;
                };
                // Empty native departure snapshots are not a new playable lobby.
                if !lobby.is_host && lobby.local().is_none() && lobby.empty() {
                    return false;
                }
                lobby.lobby_name != self.config.name(self.number)
                    || !protocol::same_map_path(
                        &lobby.map_data.map_path,
                        &protocol::client_path(&self.config.maps[self.map_index]),
                    )
                    || lobby.local().is_some_and(|p| !p.is_observer)
            }
            _ => false,
        }
    }
}

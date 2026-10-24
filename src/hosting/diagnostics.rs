//! Bounded control-journal facts, without account data or raw wire payloads.
use crate::protocol::Command;
use serde_json::Value;

pub(super) fn command(command: &Command) -> Option<String> {
    if !matches!(
        command.message,
        "InitializeNetProvider"
            | "CreateLobby"
            | "LeaveGame"
            | "LobbyCancel"
            | "LobbyStart"
            | "SetTeam"
            | "CloseSlot"
            | "OpenSlot"
            | "SetComputerSlot"
            | "ScoreScreenClose"
    ) {
        return None;
    }
    let mut text = format!("Sent {}", command.message);
    for (field, limit) in [("slot", 23), ("team", 24)] {
        if let Some(value) = command.payload[field]
            .as_u64()
            .filter(|value| *value <= limit)
        {
            text.push_str(&format!(" {field}={value}"));
        }
    }
    Some(text)
}

pub(super) fn callback(event: &Value) -> Option<String> {
    match event["messageType"].as_str()? {
        "LobbyCountdown" => {
            let flag = |name| match event["payload"]["details"][name].as_bool() {
                Some(true) => "true",
                Some(false) => "false",
                None => "unknown",
            };
            Some(format!(
                "Received LobbyCountdown active={} locked={}",
                flag("countdownActive"),
                flag("countdownLocked")
            ))
        }
        "MultiplayerGameLeave" => Some("Received MultiplayerGameLeave".into()),
        "MultiplayerGameCreateStarted" => Some("Received MultiplayerGameCreateStarted".into()),
        "MultiplayerGameCreateResult" => {
            let success = match &event["payload"]["details"]["success"] {
                value if *value == true || *value == 1 => "success",
                value if *value == false || *value == 0 => "failure",
                _ => "unknown",
            };
            Some(format!("Received MultiplayerGameCreateResult {success}"))
        }
        "SetGlueScreen" => {
            let screen = match event["payload"]["screen"].as_str()? {
                known @ ("LOGIN_DOORS"
                | "LOGIN"
                | "MAIN_MENU"
                | "CUSTOM_LOBBIES"
                | "MULTIPLAYER_CUSTOM_GAMES_SETUP"
                | "GAME_LOBBY"
                | "LOADING_SCREEN"
                | "SCORE_SCREEN") => known,
                _ => return None,
            };
            Some(format!("Received SetGlueScreen {screen}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnostic_facts_cannot_include_names_credentials_or_chat() {
        let payload = json!({"gameName":"private lobby","password":"private secret",
            "user":{"battleTag":"private account"},"slot":"private slot","team":99});
        assert_eq!(
            command(&Command::new("CreateLobby", payload.clone())).as_deref(),
            Some("Sent CreateLobby")
        );
        assert_eq!(
            command(&Command::new("SendChatMessage", payload.clone())),
            None
        );
        assert_eq!(
            callback(&json!({"messageType":"ShowModal","payload":payload})),
            None
        );
        assert_eq!(
            callback(
                &json!({"messageType":"SetGlueScreen","payload":{"screen":"private account"}})
            ),
            None
        );
        assert_eq!(
            callback(&json!({"messageType":"MultiplayerGameLeave","payload":payload})).as_deref(),
            Some("Received MultiplayerGameLeave")
        );
    }

    #[test]
    fn slot_actions_and_creation_results_have_precise_allowlisted_facts() {
        assert_eq!(
            command(&Command::new("SetTeam", json!({"slot":0,"team":24}))).as_deref(),
            Some("Sent SetTeam slot=0 team=24")
        );
        assert_eq!(
            command(&Command::new("CloseSlot", json!({"slot":17}))).as_deref(),
            Some("Sent CloseSlot slot=17")
        );
        for (success, expected) in [
            (json!(true), "success"),
            (json!(0), "failure"),
            (json!("private"), "unknown"),
        ] {
            assert_eq!(callback(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":success}}})).unwrap(), format!("Received MultiplayerGameCreateResult {expected}"));
        }
    }

    #[test]
    fn countdown_diagnostics_only_expose_boolean_state() {
        assert_eq!(
            callback(
                &json!({"messageType":"LobbyCountdown","payload":{"details":{
                    "countdownActive":true,"countdownLocked":false,"name":"private"
                }}})
            )
            .as_deref(),
            Some("Received LobbyCountdown active=true locked=false")
        );
        assert_eq!(
            callback(
                &json!({"messageType":"LobbyCountdown","payload":{"details":{
                    "countdownActive":"private","countdownLocked":42
                }}})
            )
            .as_deref(),
            Some("Received LobbyCountdown active=unknown locked=unknown")
        );
    }
}

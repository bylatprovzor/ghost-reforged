//! Allowlisted native evidence; never log raw events, accounts or session URLs.
use crate::protocol::{self, Lobby};
use serde_json::{Value, json};

pub fn summary(event: &Value) -> Result<Option<Value>, serde_json::Error> {
    let kind = event["messageType"].as_str().unwrap_or("");
    let payload = &event["payload"];
    let detail = match kind {
        "GameLobbySetup" => {
            let lobby: Lobby = serde_json::from_value(payload.clone())?;
            let slots: Vec<_> = lobby
                .players
                .iter()
                .map(|p| {
                    json!({
                        "slot": p.slot,
                        "status": p.slot_status,
                        "type": p.slot_type,
                        "team": p.team,
                        "is_self": p.is_self,
                        "is_observer": p.is_observer,
                        "team_change_enabled": p.team_change_enabled,
                    })
                })
                .collect();
            json!({
                "is_host": lobby.is_host,
                "playing_humans_without_host": lobby.humans(false),
                "local_player_present": lobby.local().is_some(),
                "slots": slots,
            })
        }
        "OnNetProviderChanged" | "OnNetProviderInitialized" => {
            json!({"bnet_confirmed": protocol::bnet_confirmation(event)})
        }
        "MultiplayerGameCreateResult" => json!({
            "success": payload["details"]["success"] == true
                || payload["details"]["success"] == 1,
        }),
        "MultiplayerGameCreateStarted"
        | "MultiplayerGameLeave"
        | "LoggedOut"
        | "GameClosed"
        | "ExitedGame" => json!({}),
        _ => return Ok(None),
    };
    Ok(Some(json!({"event": kind, "state": detail})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_lobby_evidence_excludes_names_paths_and_extra_fields() {
        let event = json!({"messageType":"GameLobbySetup", "payload":{
            "isHost":true,"lobbyName":"private lobby name",
            "mapData":{"mapPath":"private map path"},
            "account":"private account", "token":"private token",
            "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":0,
                "isSelf":true,"isObserver":false,"teamChangeEnabled":true,
                "name":"private player name"}]
        }});
        let output = summary(&event).unwrap().unwrap();
        assert_eq!(output["state"]["local_player_present"], true);
        assert_eq!(output["state"]["slots"][0]["status"], 2);
        assert!(!output.to_string().contains("private"));
        assert!(
            summary(&json!({"messageType":"ChatMessage", "payload":{
            "text":"private chat"}}))
            .unwrap()
            .is_none()
        );
    }
}

//! A failed native session authorizes recovery only for the exact empty lobby we requested.
use crate::protocol::{self, Command, Lobby};
use serde_json::Value;

pub(super) struct Request {
    name: String,
    map: String,
    started: bool,
    rejected: bool,
}

impl Request {
    pub fn from_command(command: &Command) -> Option<Self> {
        (command.message == "CreateLobby").then_some(())?;
        Some(Self {
            name: command.payload["gameName"].as_str()?.into(),
            map: command.payload["filename"].as_str()?.into(),
            started: false,
            rejected: false,
        })
    }

    pub fn observe(&mut self, event: &Value) {
        match event["messageType"].as_str() {
            Some("MultiplayerGameCreateStarted") => self.started = true,
            Some("MultiplayerGameCreateResult") if self.started => {
                let success = &event["payload"]["details"]["success"];
                self.rejected |= success == false || success == 0;
            }
            // The server can drop authentication during creation and still
            // publish its stale roster afterward, without a create-result error.
            Some("LoggedOut") if self.started => self.rejected = true,
            _ => {}
        }
    }

    pub fn matches(&self, lobby: &Lobby) -> bool {
        self.rejected
            && lobby.is_host
            && lobby.lobby_name == self.name
            && protocol::same_map_path(&lobby.map_data.map_path, &self.map)
            && lobby.empty()
            && lobby.players.iter().filter(|p| p.is_self).count() == 1
            && lobby
                .local()
                .is_some_and(|p| p.slot_status == 2 && p.slot_type == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sign_out_during_creation_can_recover_only_its_empty_matching_roster() {
        let command = Command::new(
            "CreateLobby",
            json!({"gameName":"Test #1","filename":"map.w3x"}),
        );
        let mut request = Request::from_command(&command).unwrap();
        let mut lobby: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"slot":0,"slotStatus":2,"slotType":0,"team":0,"isSelf":true,"isObserver":false,"teamChangeEnabled":true}
        ]})).unwrap();
        request.observe(&json!({"messageType":"LoggedOut"}));
        assert!(!request.matches(&lobby));
        request.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        assert!(!request.matches(&lobby));
        request.observe(&json!({"messageType":"LoggedOut"}));
        assert!(request.matches(&lobby));
        lobby.lobby_name = "Other #1".into();
        assert!(!request.matches(&lobby));
        lobby.lobby_name = "Test #1".into();
        let mut guest = lobby.players[0].clone();
        guest.slot = 1;
        guest.is_self = false;
        guest.is_observer = true;
        lobby.players.push(guest);
        assert!(!request.matches(&lobby));
    }

    #[test]
    fn recovery_requires_a_sent_started_rejected_request_and_an_empty_matching_host() {
        let command = Command::new(
            "CreateLobby",
            json!({"gameName":"Test #1","filename":"map.w3x"}),
        );
        let mut request = Request::from_command(&command).unwrap();
        let setup = json!({"isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":[
            {"slot":0,"slotStatus":2,"slotType":0,"team":0,"isSelf":true,"isObserver":false,"teamChangeEnabled":true}
        ]});
        let lobby = serde_json::from_value(setup.clone()).unwrap();
        let rejected = json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}});
        request.observe(&rejected);
        assert!(!request.matches(&lobby));
        request.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        for success in [json!(true), json!("false"), json!(null)] {
            request.observe(&json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":success}}}));
            assert!(!request.matches(&lobby));
        }
        request.observe(&rejected);
        assert!(request.matches(&lobby));
        for (pointer, value) in [
            ("/isHost", json!(false)),
            ("/lobbyName", json!("Other #1")),
            ("/mapData/mapPath", json!("other.w3x")),
            ("/players/0/isSelf", json!(false)),
            ("/players/0/slotStatus", json!(0)),
            ("/players/0/slotType", json!(1)),
        ] {
            let mut changed = setup.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(
                !request.matches(&serde_json::from_value(changed).unwrap()),
                "{pointer}"
            );
        }
        for observer in [false, true] {
            let mut occupied = setup.clone();
            let mut other = occupied["players"][0].clone();
            other["slot"] = json!(1);
            other["isSelf"] = json!(false);
            other["isObserver"] = json!(observer);
            occupied["players"].as_array_mut().unwrap().push(other);
            assert!(!request.matches(&serde_json::from_value(occupied).unwrap()));
        }
        assert!(!Request::from_command(&command).unwrap().matches(&lobby));
    }
}

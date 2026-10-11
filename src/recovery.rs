//! Startup recovery keeps an unrelated or still-playing session intact.
use crate::{
    config::Config,
    protocol::{self, Lobby},
};

pub fn resumable_lobby(config: &Config, lobby: &Lobby) -> Option<(u32, usize)> {
    let number = config.first_number.checked_sub(1).filter(|n| *n > 0)?;
    let map_index = config.maps.iter().position(|map| {
        protocol::same_map_path(&lobby.map_data.map_path, &protocol::client_path(map))
    })?;
    (config.resume_lobby
        && lobby.is_host
        && lobby.lobby_name == config.name(number)
        && lobby
            .local()
            .is_some_and(|p| p.is_observer && p.slot_type == 0 && p.slot_status == 2))
    .then_some((number, map_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn resuming_requires_opt_in_and_the_previous_profile_lobby_with_host_authority() {
        let mut config = Config {
            first_number: 8,
            maps: vec!["map.w3x".into()],
            lobby_name: "Test #{n}".into(),
            ..Config::default()
        };
        let mut lobby: Lobby = serde_json::from_value(json!({"isHost":true,"lobbyName":"Test #7","mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,"isSelf":true,"isObserver":true,"teamChangeEnabled":true}]})).unwrap();
        assert_eq!(resumable_lobby(&config, &lobby), None);
        config.resume_lobby = true;
        assert_eq!(resumable_lobby(&config, &lobby), Some((7, 0)));
        lobby.is_host = false;
        assert_eq!(resumable_lobby(&config, &lobby), None);
        lobby.is_host = true;
        lobby.lobby_name = "Test #8".into();
        assert_eq!(resumable_lobby(&config, &lobby), None);
        lobby.lobby_name = "Test #7".into();
        lobby.map_data.map_path = "another.w3x".into();
        assert_eq!(resumable_lobby(&config, &lobby), None);
        config.maps.push("another.w3x".into());
        assert_eq!(resumable_lobby(&config, &lobby), Some((7, 1)));
    }
}

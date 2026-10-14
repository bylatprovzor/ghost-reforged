use super::*;
use serde_json::json;

pub(super) fn game() -> StartedGame {
    StartedGame {
        version: 2,
        event: "game_started".into(),
        game_id: "0123456789abcdef0123456789abcdef".into(),
        ladder_match_id: Some("0123456789abcdef0123456789abcdef".into()),
        game_number: 21,
        bot_id: "ghost-fixture".into(),
        bot_name: "GHost Reforged".into(),
        game_name: "[RANKED] Survival Chaos #21".into(),
        map_name: ladder::MAP_NAME.into(),
        map_version: "1.59".into(),
        started_at_ms: 1790000000000,
        mode: "test".into(),
        players: vec![Player {
            slot: 0,
            battletag: "Player#1234".into(),
            role: "player".into(),
            battle_net_region: Some("eu".into()),
        }],
    }
}

fn reporter(path: &std::path::Path) -> Reporter {
    let (wake, _) = mpsc::channel();
    let (_, notices) = mpsc::channel();
    Reporter {
        directory: path.into(),
        bot_id: "ghost-fixture".into(),
        mode: "test".into(),
        snapshot: None,
        wake,
        notices,
    }
}

fn lobby() -> Lobby {
    serde_json::from_value(json!({
        "isHost":true,"lobbyName":"[RANKED] Survival Chaos #21",
        "mapData":{"mapName":ladder::MAP_NAME,"mapPath":"map.w3x"},
        "players":(0..17).map(|i| json!({
            "slot":i,"team":i%4,"slotStatus":2,"slotType":if (4..16).contains(&i) {1} else {0},
            "name":format!("Player{i}#1234"),"isSelf":i==16,"isObserver":i==16,
            "teamChangeEnabled":true,"playerRegion":"eu"
        })).collect::<Vec<_>>()
    }))
    .unwrap()
}

#[test]
fn observing_or_cancelling_a_lobby_never_publishes_and_confirmed_start_keeps_four_humans() {
    let temp = tempfile::tempdir().unwrap();
    let mut recorder = reporter(temp.path());
    let lobby = lobby();
    recorder.observe(&lobby, 21, Some(&game().game_id));
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    recorder.clear();
    assert!(recorder.started(&lobby.lobby_name).is_err());
    recorder.observe(&lobby, 21, Some(&game().game_id));
    assert!(recorder.started("Different lobby").is_err());
    recorder.started(&lobby.lobby_name).unwrap();
    let saved: StartedGame = serde_json::from_slice(
        &std::fs::read(temp.path().join(format!("{}.json", game().game_id))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.players.len(), 4);
    assert_eq!(
        saved.players.iter().map(|p| p.slot).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(saved.game_name, lobby.lobby_name);
    assert_eq!(saved.ladder_match_id, game().ladder_match_id);
    assert!(saved.started_at_ms > 0);
    assert!(recorder.started(&lobby.lobby_name).is_err());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn changed_rosters_and_unseeded_starts_do_not_reuse_old_rank_links() {
    let temp = tempfile::tempdir().unwrap();
    let mut recorder = reporter(temp.path());
    let mut lobby = lobby();
    recorder.observe(&lobby, 21, Some(&game().game_id));
    lobby.players[0].name = "NewPlayer#5678".into();
    recorder.observe(&lobby, 21, None);
    let event = recorder
        .event(recorder.snapshot.as_ref().unwrap(), 1790000000000)
        .unwrap();
    assert_eq!(event.ladder_match_id, None);
    assert_eq!(event.players[0].battletag, "NewPlayer#5678");
    assert_eq!(event.game_id.len(), 32);
    lobby.players[0].name.clear();
    recorder.observe(&lobby, 21, None);
    assert!(recorder.snapshot.is_none());
}

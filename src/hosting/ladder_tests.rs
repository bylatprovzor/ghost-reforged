use super::*;
use serde_json::Value;

fn fixture() -> (Engine, Value) {
    let engine = super::super::tests::ready(true);
    let mut players: Vec<_> = (0..16)
        .map(|i| {
            json!({
                "slot":i,"slotStatus":2,"slotType":if i<4 {0}else{1},"team":i,
                "color":(i+5)%16,"handicap":100,"race":1,"id":if i<4 {i+1}else{255},
                "name":if i<4 {format!("Player{i}#1234")}else{"Computer".into()},
                "isSelf":false,"isObserver":false,"teamChangeEnabled":true,
                "colorChangeEnabled":true,"slotTypeChangeEnabled":true
            })
        })
        .collect();
    players.push(json!({"slot":16,"slotStatus":2,"slotType":0,"team":24,
        "isSelf":true,"isObserver":true,"teamChangeEnabled":true,"name":"Ghost"}));
    (
        engine,
        json!({"isHost":true,"playerHost":"Ghost","lobbyName":"Test #1",
        "mapData":{"mapPath":"C:/Maps/test.w3x","mapName":ladder::MAP_NAME},"players":players}),
    )
}

fn receive(seed: &mut Seed, engine: &mut Engine, payload: &Value, now: u64) {
    seed.observe();
    engine.set_ladder_ready(seed.ready(&serde_json::from_value(payload.clone()).unwrap()));
    engine.on_event(
        &json!({"messageType":"GameLobbySetup","payload":payload}),
        now,
    );
}

fn apply(payload: &mut Value, commands: Vec<Command>, announcements: &mut Vec<String>) {
    for command in commands {
        if command.message == "SendGameChatMessage" {
            announcements.push(command.payload["content"].as_str().unwrap().to_owned());
            continue;
        }
        let Some(slot) = command.payload["slot"].as_u64() else {
            continue;
        };
        let rows = payload["players"].as_array_mut().unwrap();
        let row = rows.iter_mut().find(|r| r["slot"] == slot).unwrap();
        let key = match command.message {
            "SetHandicap" => "handicap",
            "SetRace" => "race",
            "SetColor" => "color",
            "SetTeam" => "team",
            "SetComputerSlot" => {
                row["name"] = json!(format!("Computer({})", command.payload["difficulty"]));
                continue;
            }
            other => panic!("Unexpected command {other}"),
        };
        row[key] = command.payload[if key == "race" { "raceId" } else { key }].clone();
        if command.message == "SetTeam" {
            rows.sort_by_key(|r| {
                (
                    r["team"].as_u64().unwrap(),
                    r["color"].as_u64().unwrap_or(24),
                )
            });
            for (i, row) in rows.iter_mut().enumerate() {
                row["slot"] = json!(i);
            }
        }
    }
}

fn prepared() -> (Seed, Engine, Value, Vec<String>, u64) {
    prepared_version(false)
}

fn prepared_version(session: bool) -> (Seed, Engine, Value, Vec<String>, u64) {
    let (mut engine, mut payload) = fixture();
    let mut seed = Seed::default();
    let (tx, rx) = mpsc::channel();
    let lobby = serde_json::from_value(payload.clone()).unwrap();
    seed.job = Some(Job {
        key: roster_key(&lobby).unwrap(),
        rx,
    });
    let profiles = std::array::from_fn(|i| Profile {
        slot: i as u8,
        battletag: format!("Player{i}#1234"),
        wins: 258,
        losses: 42,
        rank: Some(3),
    });
    tx.send(if session {
        Packet::encode_session(&profiles, "0123456789abcde").map(|packet| ladder::Prepared {
            profiles,
            packet,
            expires_at: Some(4102444800),
        })
    } else {
        ladder::Prepared::preview(profiles)
    })
    .unwrap();
    let mut messages = Vec::new();
    for now in 1..250 {
        receive(&mut seed, &mut engine, &payload, now);
        if seed.ready(&serde_json::from_value(payload.clone()).unwrap()) {
            assert_eq!(messages.len(), 4, "all four chat lines must precede start");
            return (seed, engine, payload, messages, now);
        }
        assert!(!engine.can_start(now));
        assert!(engine.start_now(now).is_empty());
        let commands = seed.advance(&engine, now, true, true);
        apply(&mut payload, commands, &mut messages);
    }
    panic!("Seeding did not finish");
}

#[test]
fn seeding_survives_row_reordering_and_announces_every_profile_before_start() {
    let (seed, mut engine, payload, messages, now) = prepared();
    assert!(seed.ready(&serde_json::from_value(payload).unwrap()));
    assert_eq!(messages[0], "Player0#1234: Wins: 258, Loss: 42, Rank: #3");
    assert_eq!(engine.start_now(now)[0].message, "LobbyStart");
}

#[test]
fn result_ticket_uses_the_new_marker_and_gates_start_until_confirmed() {
    let (seed, mut engine, payload, _, now) = prepared_version(true);
    assert_eq!(seed.packet.as_ref().unwrap().handicaps[1], 84);
    assert!(seed.ready(&serde_json::from_value(payload).unwrap()));
    assert_eq!(engine.start_now(now)[0].message, "LobbyStart");
}

#[test]
fn an_expired_session_clears_the_marker_before_registering_again() {
    let (mut seed, mut engine, payload, _, now) = prepared_version(true);
    seed.expires_at = Some(0);
    receive(&mut seed, &mut engine, &payload, now + 1);
    assert!(!engine.can_start(now + 1));
    let commands = seed.advance(&engine, now + 1, true, true);
    assert!(seed.stage == Stage::Fetch);
    assert!(seed.packet.is_none());
    assert_eq!(commands[0].message, "SetHandicap");
    assert_eq!(commands[0].payload["handicap"], 100);
}

#[test]
fn roster_or_packet_changes_invalidate_all_start_paths() {
    for field in ["name", "handicap", "race"] {
        let (mut seed, mut engine, mut payload, _, now) = prepared();
        let rows = payload["players"].as_array_mut().unwrap();
        if field == "name" {
            rows.iter_mut().find(|p| p["team"] == 0).unwrap()[field] = json!("New#1234");
        } else {
            rows.iter_mut().find(|p| p["color"] == 4).unwrap()[field] =
                json!(if field == "race" { 32 } else { 99 });
        }
        receive(&mut seed, &mut engine, &payload, now + 1);
        assert!(!engine.can_start(now + 1));
        engine.config.auto_start = true;
        assert!(
            engine
                .tick(now + 1)
                .iter()
                .all(|c| c.message != "LobbyStart")
        );
        assert!(engine.start_now(now + 1).is_empty());
    }
}

#[test]
fn unavailable_server_clears_the_marker_and_allows_the_normal_map() {
    let (mut engine, mut payload) = fixture();
    let mut seed = Seed::default();
    let (tx, rx) = mpsc::channel();
    let lobby = serde_json::from_value(payload.clone()).unwrap();
    seed.job = Some(Job {
        key: roster_key(&lobby).unwrap(),
        rx,
    });
    tx.send(Err(ladder::Error::Unavailable)).unwrap();
    let mut messages = Vec::new();
    for now in 1..100 {
        receive(&mut seed, &mut engine, &payload, now);
        let commands = seed.advance(&engine, now, true, true);
        apply(&mut payload, commands, &mut messages);
        if seed.ready(&serde_json::from_value(payload.clone()).unwrap()) {
            assert!(seed.stage == Stage::Unavailable);
            assert_eq!(messages.len(), 1);
            return;
        }
    }
    panic!("Fallback did not become ready");
}

#[test]
fn ordinary_maps_keep_the_existing_start_behavior() {
    let mut engine = super::super::tests::ready(true);
    assert_eq!(engine.start_now(1)[0].message, "LobbyStart");
    assert!(ladder::matches_map(
        "|cFF00FF00Survival Chaos Reforged Rank|r"
    ));
    assert!(!ladder::matches_map("Survival Chaos Reborn v1.59"));
}

#[test]
fn changing_a_packet_after_a_long_wait_gets_a_fresh_repair_deadline() {
    let (mut seed, mut engine, mut payload, _, now) = prepared();
    let rows = payload["players"].as_array_mut().unwrap();
    rows.iter_mut().find(|p| p["color"] == 4).unwrap()["handicap"] = json!(99);
    let mut messages = Vec::new();
    for tick in now + 300..now + 420 {
        receive(&mut seed, &mut engine, &payload, tick);
        let commands = seed.advance(&engine, tick, true, true);
        apply(&mut payload, commands, &mut messages);
        assert!(seed.stage != Stage::Unavailable);
        if seed.ready(&serde_json::from_value(payload.clone()).unwrap()) {
            assert_eq!(messages.len(), 4);
            return;
        }
    }
    panic!("Packet repair did not finish");
}

#[test]
fn a_finished_request_for_a_previous_roster_is_discarded() {
    let (mut engine, payload) = fixture();
    let mut seed = Seed::default();
    let (tx, rx) = mpsc::channel();
    seed.job = Some(Job {
        key: "old-roster".into(),
        rx,
    });
    tx.send(Err(ladder::Error::Unavailable)).unwrap();
    receive(&mut seed, &mut engine, &payload, 1);
    seed.key = roster_key(&serde_json::from_value(payload.clone()).unwrap()).unwrap();
    seed.stage = Stage::Fetch;
    seed.advance(&engine, 1, true, true);
    assert!(seed.stage == Stage::Fetch);
    assert!(seed.job.is_none());
    assert!(seed.packet.is_none());
}

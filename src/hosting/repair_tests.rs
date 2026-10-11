use super::*;
use crate::discovery::Endpoint;

fn placeholder() -> Value {
    json!({"messageType":"GameLobbySetup","payload":{
        "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
        "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,
            "isSelf":false,"isObserver":false,"teamChangeEnabled":false}]
    }})
}

fn lobby(event: &Value) -> Lobby {
    serde_json::from_value(event["payload"].clone()).unwrap()
}

#[test]
fn incomplete_signature_excludes_real_single_slot_lobbies_and_any_occupant() {
    let original = placeholder();
    assert!(incomplete_lobby(&lobby(&original)));
    for (field, value) in [
        ("slotStatus", json!(2)),
        ("slotStatus", json!(1)),
        ("slotType", json!(1)),
        ("team", json!(0)),
        ("isSelf", json!(true)),
        ("isObserver", json!(true)),
        ("slot", json!(1)),
    ] {
        let mut changed = original.clone();
        changed["payload"]["players"][0][field] = value;
        assert!(!incomplete_lobby(&lobby(&changed)), "{field}");
    }
    let mut changed = original.clone();
    changed["payload"]["isHost"] = json!(false);
    assert!(!incomplete_lobby(&lobby(&changed)));
    changed = original.clone();
    changed["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(original["payload"]["players"][0].clone());
    assert!(!incomplete_lobby(&lobby(&changed)));
}

#[test]
fn transient_creation_placeholder_and_started_games_never_request_repair() {
    let mut repair = Repair::default();
    let event = placeholder();
    assert!(
        repair
            .observe(&Phase::Creating(0), Some(&event), 0)
            .is_none()
    );
    assert!(
        repair
            .observe(&Phase::Creating(0), Some(&event), 2)
            .is_none()
    );
    assert!(
        repair
            .observe(&Phase::Creating(0), Some(&event), 3)
            .is_some()
    );
    let mut valid = event.clone();
    valid["payload"]["players"][0]["isSelf"] = json!(true);
    valid["payload"]["players"][0]["slotStatus"] = json!(2);
    assert!(
        repair
            .observe(&Phase::Creating(0), Some(&valid), 4)
            .is_none()
    );
    assert!(
        repair
            .observe(&Phase::Creating(0), Some(&event), 5)
            .is_none()
    );
    for phase in [
        Phase::Starting(5),
        Phase::InGame,
        Phase::WatchingGame,
        Phase::Restarting,
    ] {
        assert!(repair.observe(&phase, Some(&event), 20).is_none());
    }
}

#[test]
fn loading_without_a_countdown_protects_later_placeholder_snapshots_until_menu() {
    let mut repair = Repair::default();
    let event = placeholder();
    let phase = Phase::Lobby;
    let loading = json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}});
    repair.observe(&phase, Some(&loading), 0);
    assert!(repair.observe(&phase, Some(&event), 1).is_none());
    assert!(repair.observe(&phase, Some(&event), 10).is_none());
    let menu = json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}});
    repair.observe(&phase, Some(&menu), 11);
    assert!(
        repair
            .observe(&Phase::Creating(12), Some(&event), 12)
            .is_none()
    );
    assert!(
        repair
            .observe(&Phase::Creating(12), Some(&event), 15)
            .is_some()
    );
}

fn server(responses: Vec<Vec<Value>>) -> (Client, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        for batch in responses {
            let request: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(request["message"], "SendGameLobbySetup");
            for event in batch {
                socket
                    .send(tungstenite::Message::Text(event.to_string().into()))
                    .unwrap();
            }
        }
        // Keep the transport open until the caller has observed the idle queue.
        let _ = socket.read();
    });
    let client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "test".into(),
        version: None,
    })
    .unwrap();
    (client, server)
}

#[test]
fn restart_requires_fresh_acknowledgment_and_a_second_failure_stops() {
    let event = placeholder();
    let (mut client, server) = server(vec![vec![event.clone()], vec![event.clone()]]);
    let (tx, _rx) = std::sync::mpsc::channel();
    let stop = AtomicBool::new(false);
    let mut repair = Repair::default();
    let mut pending = VecDeque::new();
    assert!(
        repair
            .confirm(&lobby(&event), &mut client, &mut pending, &tx, &stop)
            .unwrap()
    );
    assert_eq!(pending.len(), 1);
    pending.clear();
    assert!(
        repair
            .confirm(&lobby(&event), &mut client, &mut pending, &tx, &stop)
            .unwrap_err()
            .contains("after restarting")
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn startup_uses_the_latest_roster_after_a_menu_but_never_overrides_loading() {
    let event = placeholder();
    for screen in ["MAIN_MENU", "LOADING_SCREEN"] {
        let responses = if screen == "MAIN_MENU" {
            vec![vec![event.clone()]]
        } else {
            vec![]
        };
        let (mut client, server) = server(responses);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut repair = Repair::default();
        let mut pending = VecDeque::from([
            json!({"messageType":"SetGlueScreen","payload":{"screen":screen}}),
            event.clone(),
        ]);
        let result = repair
            .confirm(
                &lobby(&event),
                &mut client,
                &mut pending,
                &tx,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(result, screen == "MAIN_MENU");
        drop(client);
        server.join().unwrap();
    }
}

#[test]
fn a_new_player_or_loading_callback_after_the_placeholder_cancels_restart() {
    let event = placeholder();
    let mut occupied = event.clone();
    occupied["payload"]["players"][0]["slotStatus"] = json!(2);
    for changed in [
        occupied,
        json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}}),
        json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":true}}}),
        json!({"messageType":"GameLobbySetup","payload":{}}),
    ] {
        let (mut client, server) = server(vec![vec![event.clone(), changed.clone()]]);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut repair = Repair::default();
        let mut pending = VecDeque::new();
        assert!(
            !repair
                .confirm(
                    &lobby(&event),
                    &mut client,
                    &mut pending,
                    &tx,
                    &AtomicBool::new(false)
                )
                .unwrap()
        );
        assert!(!repair.attempted);
        assert_eq!(pending.back(), Some(&changed));
        drop(client);
        server.join().unwrap();
    }
}

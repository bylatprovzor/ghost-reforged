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
    for now in [0, 2, 3, 60, 119] {
        assert!(
            repair
                .observe_dispatched(&Phase::Creating(0), Some(&event), now)
                .is_none()
        );
    }
    assert!(
        repair
            .observe_dispatched(&Phase::Recovering(120), Some(&event), 120)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Recovering(120), Some(&event), 122)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Recovering(120), Some(&event), 123)
            .is_some()
    );
    let mut valid = event.clone();
    valid["payload"]["players"][0]["isSelf"] = json!(true);
    valid["payload"]["players"][0]["slotStatus"] = json!(2);
    assert!(
        repair
            .observe_dispatched(&Phase::Lobby, Some(&valid), 124)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Lobby, Some(&event), 125)
            .is_none()
    );
    for phase in [
        Phase::Starting(5),
        Phase::InGame,
        Phase::WatchingGame,
        Phase::Restarting,
    ] {
        assert!(
            repair
                .observe_dispatched(&phase, Some(&event), 140)
                .is_none()
        );
    }
}

#[test]
fn loading_without_a_countdown_protects_later_placeholder_snapshots_until_menu() {
    let mut repair = Repair::default();
    let event = placeholder();
    let phase = Phase::Lobby;
    let loading = json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}});
    repair.inspect_pending(&mut loading.clone().into());
    repair.observe_dispatched(&phase, Some(&loading), 0);
    assert!(repair.observe_dispatched(&phase, Some(&event), 1).is_none());
    assert!(
        repair
            .observe_dispatched(&phase, Some(&event), 10)
            .is_none()
    );
    let menu = json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}});
    repair.inspect_pending(&mut menu.clone().into());
    repair.observe_dispatched(&phase, Some(&menu), 11);
    assert!(
        repair
            .observe_dispatched(&Phase::Creating(12), Some(&event), 12)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Creating(12), Some(&event), 15)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Recovering(132), Some(&event), 132)
            .is_none()
    );
    assert!(
        repair
            .observe_dispatched(&Phase::Recovering(132), Some(&event), 135)
            .is_some()
    );
}

#[test]
fn reconnect_repair_cannot_overtake_a_pending_creation_deadline() {
    let event = placeholder();
    let (mut client, server) = server(vec![vec![event.clone()]]);
    let (tx, _rx) = std::sync::mpsc::channel();
    let stop = AtomicBool::new(false);
    let mut repair = Repair::default();
    let mut pending = VecDeque::new();
    repair.creation_started(Duration::from_secs(120));
    for now in [30, 60, 119] {
        assert!(
            repair
                .observe_dispatched(&Phase::Recovering(30), Some(&event), now)
                .is_none()
        );
    }
    assert!(
        !repair
            .confirm(&lobby(&event), &mut client, &mut pending, &tx, &stop)
            .unwrap()
    );
    assert!(pending.is_empty());
    repair.creating_until = Some(Instant::now() - Duration::from_secs(1));
    assert!(
        repair
            .confirm(&lobby(&event), &mut client, &mut pending, &tx, &stop)
            .unwrap()
    );
    drop(client);
    server.join().unwrap();
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

fn rejected_creation() -> (Repair, Value) {
    failed_creation(
        json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}),
    )
}

#[test]
fn fresh_or_queued_native_creation_revokes_repair_despite_a_later_placeholder() {
    for queued in [true, false] {
        let setup = placeholder();
        let events = vec![
            json!({"messageType":"MultiplayerGameCreateStarted"}),
            setup.clone(),
        ];
        let (mut client, worker) = server(if queued { vec![] } else { vec![events.clone()] });
        let mut repair = Repair::default();
        let mut pending = if queued {
            events.into_iter().map(Into::into).collect()
        } else {
            VecDeque::new()
        };
        let (tx, _) = std::sync::mpsc::channel();
        assert!(
            !repair
                .confirm(
                    &lobby(&setup),
                    &mut client,
                    &mut pending,
                    &tx,
                    &AtomicBool::new(false)
                )
                .unwrap()
        );
        assert!(repair.protected);
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn a_new_creation_veto_survives_handshake_loss_without_counting_queue_replay() {
    let (mut repair, setup) = rejected_creation();
    // The original callback may have been consumed during the handshake.
    repair.native_creation_started = true;
    let mut next = super::super::pending::Callback::from(
        json!({"messageType":"MultiplayerGameCreateStarted"}),
    );
    repair.inspect_pending(&mut next);
    assert!(repair.protected);
    // Drop the callback as a failed handshake does, then reconnect with only a
    // matching placeholder. Neither inspection nor confirmation may restore authority.
    drop(next);
    let mut later = super::super::pending::Callback::from(setup.clone());
    repair.inspect_pending(&mut later);
    let (mut client, worker) = server(vec![]);
    let (tx, _) = std::sync::mpsc::channel();
    assert!(
        !repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut VecDeque::from([later]),
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();

    let mut repair = Repair::default();
    let command = crate::protocol::Command::new(
        "CreateLobby",
        json!({"gameName":"Test #1","filename":"map.w3x"}),
    );
    repair.creation_sent(super::super::creation_recovery::Request::from_command(&command).unwrap());
    let mut first = super::super::pending::Callback::from(
        json!({"messageType":"MultiplayerGameCreateStarted"}),
    );
    repair.inspect_pending(&mut first);
    repair.inspect_pending(&mut first);
    assert!(
        !repair.protected,
        "Dispatching the same callback is not another native start"
    );
    // Replaying the expected start after the handshake must not revoke a later
    // explicit failure of that same request or prevent its fresh confirmation.
    let mut failure = super::super::pending::Callback::from(
        json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}),
    );
    repair.inspect_pending(&mut failure);
    let setup = placeholder();
    let mut received_setup = super::super::pending::Callback::from(setup.clone());
    repair.inspect_pending(&mut received_setup);
    let (mut client, worker) = server(vec![vec![setup.clone()]]);
    let (tx, _) = std::sync::mpsc::channel();
    assert!(
        repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut VecDeque::from([first, failure, received_setup]),
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();
}

#[test]
fn dispatching_old_menu_cannot_erase_a_newer_creation_veto() {
    let (mut repair, setup) = rejected_creation();
    repair.native_creation_started = true;
    repair.creating_until = None;
    let mut pending: VecDeque<super::super::pending::Callback> = [
        json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}),
        json!({"messageType":"MultiplayerGameCreateStarted"}),
        setup.clone(),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    for event in &mut pending {
        repair.inspect_pending(event);
    }
    assert!(repair.protected);
    // Exact controller ordering: receipt/handshake, then delayed dispatch.
    for (index, mut event) in pending.into_iter().enumerate() {
        repair.inspect_pending(&mut event);
        assert!(
            repair
                .observe_dispatched(&Phase::Recovering(0), Some(&event.value), index as u64 + 20)
                .is_none()
        );
        assert!(repair.protected);
    }
    let mut later = super::super::pending::Callback::from(setup.clone());
    repair.inspect_pending(&mut later);
    let (mut client, worker) = server(vec![]);
    let (tx, _) = std::sync::mpsc::channel();
    assert!(
        !repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut VecDeque::from([later]),
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();
}

#[test]
fn a_repair_transport_error_does_not_discard_pending_recovery_protection() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let _ = socket.read().unwrap();
        socket
            .send(tungstenite::Message::Text(
                json!({"messageType":"MultiplayerGameJoinStarted"})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        socket.close(None).unwrap();
    });
    let mut client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "fixture".into(),
        version: None,
    })
    .unwrap();
    let mut recovery = super::super::login_recovery::Recovery::default();
    recovery.creation_requested();
    recovery.observe(
        &json!({"messageType":"MultiplayerGameCreateStarted"}),
        Instant::now(),
    );
    recovery.observe(
        &json!({"messageType":"MultiplayerGameLeave"}),
        Instant::now(),
    );
    let mut repair = Repair::default();
    let (tx, _) = std::sync::mpsc::channel();
    let result = repair.confirm_with_recovery(
        &lobby(&placeholder()),
        &mut client,
        &mut VecDeque::new(),
        &tx,
        &AtomicBool::new(false),
        &mut recovery,
    );
    assert!(result.is_err());
    let diagnostic = recovery.diagnostic(0).unwrap();
    assert!(diagnostic.contains("eligible=false"), "{diagnostic}");
    assert!(
        diagnostic.contains("blocked_by=a game join"),
        "{diagnostic}"
    );
    drop(client);
    worker.join().unwrap();
}

fn failed_creation(failure: Value) -> (Repair, Value) {
    let mut repair = Repair::default();
    let command = crate::protocol::Command::new(
        "CreateLobby",
        json!({"gameName":"Test #1","filename":"map.w3x"}),
    );
    repair.creation_started(Duration::from_secs(180));
    repair.creation_sent(super::super::creation_recovery::Request::from_command(&command).unwrap());
    repair.inspect(&json!({"messageType":"MultiplayerGameCreateStarted"}));
    repair.inspect(&failure);
    let mut setup = placeholder();
    setup["payload"]["players"][0]["isSelf"] = json!(true);
    setup["payload"]["players"][0]["slotStatus"] = json!(2);
    setup["payload"]["players"][0]["team"] = json!(0);
    (repair, setup)
}

#[test]
fn rejected_populated_lobby_requires_a_fresh_empty_confirmation() {
    let (mut repair, setup) = rejected_creation();
    assert!(repair.rejected_lobby(&lobby(&setup)));
    let (mut client, server) = server(vec![vec![setup.clone()]]);
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut VecDeque::new(),
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    drop(client);
    server.join().unwrap();
}

#[test]
fn sign_out_recovery_preserves_callbacks_and_cannot_cross_loading() {
    let logout = json!({"messageType":"LoggedOut"});
    let (mut repair, setup) = failed_creation(logout.clone());
    let (mut client, server) = server(vec![vec![setup.clone()]]);
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut pending = VecDeque::from([logout.clone().into(), setup.clone().into()]);
    assert!(
        repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut pending,
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    assert_eq!(pending.front().map(|event| &event.value), Some(&logout));
    assert_eq!(pending.back().map(|event| &event.value), Some(&setup));
    drop(client);
    server.join().unwrap();
    for screen in ["LOADING_SCREEN", "SCORE_SCREEN"] {
        let (mut repair, setup) = failed_creation(logout.clone());
        repair.inspect(&json!({"messageType":"SetGlueScreen","payload":{"screen":screen}}));
        repair.inspect(&logout);
        repair.inspect(&json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}));
        assert!(!repair.rejected_lobby(&lobby(&setup)));
    }
}

#[test]
fn a_join_loading_or_new_request_revokes_failed_creation_reset_authority() {
    let (mut dead_client, previous_lobby) = rejected_creation();
    dead_client.forget_creation();
    assert!(!dead_client.rejected_lobby(&lobby(&previous_lobby)));
    let (mut repair, setup) = rejected_creation();
    let mut joined = setup.clone();
    let mut other = joined["payload"]["players"][0].clone();
    other["isSelf"] = json!(false);
    other["slot"] = json!(1);
    joined["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(other);
    let (mut client, server) = server(vec![vec![joined.clone()]]);
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut pending = VecDeque::new();
    assert!(
        !repair
            .confirm(
                &lobby(&setup),
                &mut client,
                &mut pending,
                &tx,
                &AtomicBool::new(false)
            )
            .unwrap()
    );
    assert_eq!(pending.back().map(|event| &event.value), Some(&joined));
    drop(client);
    server.join().unwrap();
    for event in [
        json!({"messageType":"SetGlueScreen","payload":{"screen":"LOADING_SCREEN"}}),
        json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":true}}}),
    ] {
        let (mut repair, setup) = rejected_creation();
        repair.inspect(&event);
        repair.inspect(&json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}));
        assert!(!repair.rejected_lobby(&lobby(&setup)));
    }
    let (mut repair, setup) = rejected_creation();
    let command = crate::protocol::Command::new(
        "CreateLobby",
        json!({"gameName":"Test #2","filename":"map.w3x"}),
    );
    repair.creation_sent(super::super::creation_recovery::Request::from_command(&command).unwrap());
    assert!(!repair.rejected_lobby(&lobby(&setup)));
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
            json!({"messageType":"SetGlueScreen","payload":{"screen":screen}}).into(),
            event.clone().into(),
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
        assert_eq!(pending.back().map(|event| &event.value), Some(&changed));
        drop(client);
        server.join().unwrap();
    }
}

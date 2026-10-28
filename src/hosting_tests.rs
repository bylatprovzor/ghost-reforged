use super::*;
use crate::config::HostMode;

#[path = "hosting_action_tests.rs"]
mod action_tests;
#[path = "hosting_slot_tests.rs"]
mod slot_tests;

pub(super) fn event(kind: &str, payload: Value) -> Value {
    json!({"messageType":kind,"payload":payload})
}

pub(super) fn lobby(occupied: bool) -> Value {
    event(
        "GameLobbySetup",
        json!({
            "isHost":true,"playerHost":"Ghost","lobbyName":"Test #1",
            "mapData":{"mapPath":"C:/Maps/test.w3x"},"players":[
                {"slot":0,"slotStatus":2,"slotType":0,"team":24,
                 "isSelf":true,"isObserver":true,"teamChangeEnabled":true,"name":"Ghost"},
                {"slot":1,"slotStatus":if occupied {2} else {0},"slotType":0,"team":0,
                 "isSelf":false,"isObserver":false,"teamChangeEnabled":true,"name":"Player"}
            ]
        }),
    )
}

pub(super) fn ready(occupied: bool) -> Engine {
    let mut engine = Engine::new(Config {
        maps: vec!["C:/Maps/test.w3x".into()],
        host_mode: HostMode::Observer,
        lobby_name: "Test #{n}".into(),
        auto_start: false,
        countdown_seconds: 0,
        ..Config::default()
    });
    engine.on_event(&event("GameList", json!({})), 0);
    engine.on_event(
        &event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        0,
    );
    engine.on_event(
        &event(
            "MultiplayerGameCreateResult",
            json!({"details":{"success":true}}),
        ),
        0,
    );
    engine.on_event(&lobby(occupied), 0);
    assert_eq!(engine.phase, Phase::Lobby);
    engine
}

#[test]
fn countdown_rosters_are_refreshed_without_waiting_for_the_idle_poll_interval() {
    let mut engine = ready(true);
    assert_eq!(snapshot_interval(&engine), Duration::from_secs(2));
    engine.phase = Phase::Countdown(5);
    assert_eq!(snapshot_interval(&engine), Duration::from_millis(50));
    engine.start_now(1);
    assert_eq!(snapshot_interval(&engine), Duration::from_millis(50));
    engine.phase = Phase::InGame;
    assert_eq!(snapshot_interval(&engine), Duration::from_secs(2));
}

#[test]
fn help_and_counts_remain_available_during_native_countdown_without_accepting_votes() {
    let mut engine = ready(true);
    let mut setup = lobby(true);
    setup["payload"]["players"][0]["name"] = json!("");
    engine.on_event(&setup, 1);
    engine.start_now(1);
    let mut chat = crate::commands::LobbyCommands::default();
    for (offset, command) in [
        "?help",
        "?players",
        "?votestart",
        "?votecancel",
        "?votekick Player",
    ]
    .into_iter()
    .enumerate()
    {
        let message = event(
            "ChatMessage",
            json!({"message":{
                "source":"gameChat","sender":"Player","content":command
            }}),
        );
        let outcome = control::chat_command(
            &engine,
            &mut chat,
            Some(&message),
            true,
            Duration::from_secs(2 + offset as u64),
            None,
        );
        assert!(!outcome.start && outcome.kick.is_none());
        assert_eq!(outcome.reply.is_some(), offset < 2, "{command}");
        if command == "?help" {
            let reply = outcome.reply.unwrap();
            let content = reply.payload["content"].as_str().unwrap();
            assert!(content.starts_with("Commands: "));
            assert!(content.contains("?help") && content.contains("?players"));
            assert!(!content.contains("?votestart"));
        }
    }
    let message = event(
        "ChatMessage",
        json!({"message":{
            "source":"gameChat","sender":"Player","content":"?help"
        }}),
    );
    assert!(
        control::chat_command(
            &engine,
            &mut chat,
            Some(&message),
            false,
            Duration::from_secs(7),
            None
        )
        .reply
        .is_none()
    );
    engine
        .config
        .commands
        .set(crate::commands::ChatCommand::Help, false);
    assert!(
        control::chat_command(
            &engine,
            &mut chat,
            Some(&message),
            true,
            Duration::from_secs(7),
            None
        )
        .reply
        .is_none()
    );
    engine
        .config
        .commands
        .set(crate::commands::ChatCommand::Help, true);
    engine.phase = Phase::InGame;
    assert!(
        control::chat_command(
            &engine,
            &mut chat,
            Some(&message),
            true,
            Duration::from_secs(7),
            None
        )
        .reply
        .is_none()
    );
}

#[test]
fn a_departure_received_with_start_prevents_the_start_request() {
    let mut engine = ready(true);
    let commands = dispatch(
        &mut engine,
        Some(&lobby(false)),
        Some(&Control::Start),
        true,
        1,
    );
    assert!(
        commands.is_empty(),
        "A departure must be processed before Start"
    );
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn a_join_received_with_close_does_not_close_the_occupied_slot() {
    let mut engine = ready(false);
    let commands = dispatch(
        &mut engine,
        Some(&lobby(true)),
        Some(&Control::Slot {
            slot: 1,
            open: false,
        }),
        true,
        1,
    );
    assert!(
        commands.is_empty(),
        "A joined player must never be closed out"
    );
}

#[test]
fn lost_authority_received_with_start_prevents_the_start_request() {
    let mut engine = ready(true);
    let mut changed = lobby(true);
    changed["payload"]["isHost"] = json!(false);
    let commands = dispatch(&mut engine, Some(&changed), Some(&Control::Start), true, 1);
    assert!(commands.is_empty());
    assert!(matches!(engine.phase, Phase::Stopped(_)));
}

#[test]
fn an_unverified_alias_blocks_manual_and_automatic_start() {
    for automatic in [false, true] {
        let mut engine = ready(true);
        engine.config.auto_start = automatic;
        engine.config.min_players = 1;
        assert!(
            dispatch(
                &mut engine,
                Some(&lobby(true)),
                Some(&Control::Start),
                false,
                1
            )
            .is_empty()
        );
        assert!(dispatch(&mut engine, None, None, false, 2).is_empty());
        assert_eq!(engine.config.auto_start, automatic);
        assert_eq!(engine.phase, Phase::Lobby);
    }
}

#[test]
fn live_start_rule_changes_are_applied_before_the_next_native_event() {
    let mut engine = ready(true);
    engine.config.auto_start = true;
    engine.config.min_players = 1;
    let control = Control::StartRule {
        minimum: 3,
        full: false,
    };
    assert!(dispatch(&mut engine, Some(&lobby(true)), Some(&control), true, 1).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
    let control = Control::StartRule {
        minimum: 1,
        full: false,
    };
    dispatch(&mut engine, Some(&lobby(true)), Some(&control), true, 2);
    assert_eq!(
        dispatch(&mut engine, None, None, true, 2)[0].message,
        "LobbyStart"
    );
    let control = Control::StartRule {
        minimum: 3,
        full: false,
    };
    assert_eq!(
        dispatch(&mut engine, None, Some(&control), true, 3)[0].message,
        "LobbyCancel"
    );
    assert!(matches!(engine.phase, Phase::Starting(_)));
    let cancelled = event(
        "LobbyCountdown",
        json!({"details":{
            "countdownActive":false,"countdownLocked":false
        }}),
    );
    assert!(dispatch(&mut engine, Some(&cancelled), None, true, 3).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn a_fresh_verified_lobby_starts_once_and_empty_slots_remain_controllable() {
    let mut engine = ready(false);
    let commands = dispatch(
        &mut engine,
        Some(&lobby(false)),
        Some(&Control::Slot {
            slot: 1,
            open: false,
        }),
        true,
        1,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "CloseSlot");
    let commands = dispatch(
        &mut engine,
        Some(&lobby(true)),
        Some(&Control::Start),
        true,
        2,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "LobbyStart");
    assert!(dispatch(&mut engine, None, Some(&Control::Start), true, 3).is_empty());
}

#[test]
fn stopping_during_number_reservation_is_not_a_failure() {
    for already_cancelled in [false, true] {
        let (commands, rx) = mpsc::channel();
        let (tx, events) = mpsc::channel();
        if !already_cancelled {
            commands.send(Control::Stop).unwrap();
        }
        assert_eq!(
            reserve(
                1,
                &rx,
                &tx,
                &AtomicBool::new(already_cancelled),
                &mut VecDeque::new(),
                Duration::from_secs(120)
            )
            .unwrap(),
            Reservation::Cancelled
        );
        assert!(
            events
                .try_iter()
                .all(|event| matches!(event, Event::Reserve(1)))
        );
    }
}

#[test]
fn a_failed_number_save_still_stops_creation() {
    let (commands, rx) = mpsc::channel();
    let (tx, _events) = mpsc::channel();
    commands
        .send(Control::Reserved {
            number: 1,
            saved: false,
        })
        .unwrap();
    assert!(
        reserve(
            1,
            &rx,
            &tx,
            &AtomicBool::new(false),
            &mut VecDeque::new(),
            Duration::from_secs(120)
        )
        .is_err()
    );
}

#[test]
fn reservation_preserves_settings_received_before_the_acknowledgment() {
    let (commands, rx) = mpsc::channel();
    let (tx, _events) = mpsc::channel();
    commands
        .send(Control::StartRule {
            minimum: 7,
            full: false,
        })
        .unwrap();
    commands
        .send(Control::Reserved {
            number: 1,
            saved: true,
        })
        .unwrap();
    let mut deferred = VecDeque::new();
    assert_eq!(
        reserve(
            1,
            &rx,
            &tx,
            &AtomicBool::new(false),
            &mut deferred,
            Duration::from_secs(120)
        )
        .unwrap(),
        Reservation::Saved
    );
    assert!(matches!(
        deferred.pop_front(),
        Some(Control::StartRule {
            minimum: 7,
            full: false
        })
    ));
}

#[test]
fn slow_durable_counter_acknowledgment_is_awaited_before_creating() {
    let (commands, rx) = mpsc::channel();
    let (tx, events) = mpsc::channel();
    let frontend = thread::spawn(move || {
        assert!(matches!(events.recv().unwrap(), Event::Reserve(1)));
        thread::sleep(Duration::from_secs(6));
        commands
            .send(Control::Reserved {
                number: 1,
                saved: true,
            })
            .unwrap();
    });
    assert_eq!(
        reserve(
            1,
            &rx,
            &tx,
            &AtomicBool::new(false),
            &mut VecDeque::new(),
            Duration::from_secs(10)
        )
        .unwrap(),
        Reservation::Saved
    );
    frontend.join().unwrap();
}

#[test]
fn shutdown_tracks_owned_stale_lobbies_but_not_a_running_game() {
    let mut engine = ready(true);
    assert!(engine.owned_lobby(100).is_none());
    assert_eq!(engine.shutdown_identity().unwrap().0, "Test #1");
    engine.phase = Phase::Leaving(99);
    assert_eq!(engine.shutdown_identity().unwrap().0, "Test #1");
    engine.phase = Phase::Lobby;
    engine.on_event(&lobby(true), 100);
    engine.start_now(100);
    assert!(engine.shutdown_identity().is_some());
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        101,
    );
    assert!(engine.shutdown_identity().is_none());
}

#[test]
fn live_settings_change_the_current_lobby_without_recreating_it() {
    let mut engine = ready(true);
    let settings = LiveSettings {
        headless: false,
        minimum: 3,
        full: false,
        rehost: false,
        reuse_session: false,
        rehost_delay: 12,
        empty_refresh: 60,
        commands: Default::default(),
        welcome: crate::welcome::Welcome {
            enabled: true,
            message: "Hello #{playername}".into(),
            second: String::new(),
            whisper: false,
            min_mmr: None,
            max_mmr: None,
        },
    };
    let commands = dispatch(
        &mut engine,
        Some(&lobby(true)),
        Some(&Control::Settings(settings.clone())),
        true,
        1,
    );
    assert!(!commands.iter().any(|c| c.message == "CreateLobby"));
    assert_eq!(engine.config.welcome, settings.welcome);
    assert!(!engine.config.restart_on_load);
    assert_eq!(engine.config.empty_rehost_seconds, 60);
    let mut invalid = settings.clone();
    invalid.minimum = 0;
    invalid.welcome.message = "Should not apply".into();
    dispatch(
        &mut engine,
        None,
        Some(&Control::Settings(invalid)),
        true,
        2,
    );
    assert_eq!(engine.config.welcome, settings.welcome);
}

#[test]
fn published_controls_follow_countdown_without_another_roster_callback() {
    let mut engine = ready(true);
    let (tx, events) = mpsc::channel();
    let mut previous = None;
    publish_lobby(&engine, true, 0, &mut previous, &tx);
    let Event::Lobby(open) = events.recv().unwrap() else {
        panic!("Missing lobby")
    };
    assert!(open.can_start && open.can_edit_slots);
    engine.start_now(1);
    publish_lobby(&engine, true, 1, &mut previous, &tx);
    let Event::Lobby(starting) = events.recv().unwrap() else {
        panic!("Missing countdown state")
    };
    assert!(!starting.can_start && !starting.can_edit_slots);
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        2,
    );
    publish_lobby(&engine, true, 2, &mut previous, &tx);
    assert!(matches!(events.recv().unwrap(), Event::LobbyCleared));
    assert!(previous.is_none());
}

#[test]
fn stale_or_unverified_lobbies_are_removed_before_accepting_more_controls() {
    for (alias_ready, now) in [(false, 1), (true, 8)] {
        let engine = ready(true);
        let (tx, events) = mpsc::channel();
        let mut previous = None;
        publish_lobby(&engine, true, 0, &mut previous, &tx);
        events.recv().unwrap();
        publish_lobby(&engine, alias_ready, now, &mut previous, &tx);
        assert!(matches!(events.recv().unwrap(), Event::LobbyCleared));
        assert!(previous.is_none());
        publish_lobby(&engine, alias_ready, now, &mut previous, &tx);
        assert!(events.try_recv().is_err());
    }
}

#[test]
fn player_capacity_excludes_computers_observers_and_closed_slots() {
    let engine = ready(true);
    let mut snapshot = summarize(engine.owned_lobby(0).unwrap(), true, true);
    assert_eq!(snapshot.player_capacity(), 1);
    for (status, computer, observer) in [(2, true, false), (1, false, false), (0, false, true)] {
        snapshot.slots.push(Slot {
            id: None,
            name: String::new(),
            number: snapshot.slots.len() as u8,
            status,
            computer,
            observer,
            own: false,
            change_type: true,
            change_team: true,
        });
    }
    assert_eq!(snapshot.player_capacity(), 1);
    snapshot.slots.push(Slot {
        id: None,
        name: String::new(),
        number: 5,
        status: 0,
        computer: false,
        observer: false,
        own: false,
        change_type: true,
        change_team: true,
    });
    assert_eq!(snapshot.player_capacity(), 2);
}

#[test]
fn activity_reports_names_without_inventing_departures_when_players_move() {
    let mut engine = ready(false);
    let mut activity = activity::ActivityTracker::default();
    assert_eq!(
        activity.observe(&engine, 0, true),
        vec![Activity::LobbyCreated("Test #1".into())]
    );
    assert!(activity.observe(&engine, 0, true).is_empty());
    let mut joined = lobby(true);
    joined["payload"]["players"][1]["name"] = json!("Alice");
    joined["payload"]["players"][1]["id"] = json!(42);
    engine.on_event(&joined, 1);
    assert_eq!(
        activity.observe(&engine, 1, true),
        vec![Activity::Joined("Alice".into())]
    );
    assert_eq!(
        summarize(engine.owned_lobby(1).unwrap(), true, true).slots[1].name,
        "Alice"
    );
    joined["payload"]["players"][1]["slot"] = json!(3);
    engine.on_event(&joined, 2);
    assert!(activity.observe(&engine, 2, true).is_empty());
    joined["payload"]["players"][1]["name"] = json!("Bob");
    joined["payload"]["players"][1]["id"] = json!(42);
    engine.on_event(&joined, 3);
    assert_eq!(
        activity.observe(&engine, 3, true),
        vec![
            Activity::Left("Alice".into()),
            Activity::Joined("Bob".into())
        ]
    );
    engine.start_now(3);
    assert!(activity.observe(&engine, 3, false).is_empty());
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        4,
    );
    assert_eq!(
        activity.observe(&engine, 4, false),
        vec![Activity::GameStarted("Test #1".into())]
    );
    assert!(activity.observe(&engine, 5, false).is_empty());
}

#[test]
fn native_joins_wait_for_a_name_and_keep_it_through_transient_empty_updates() {
    let mut engine = ready(false);
    let mut activity = activity::ActivityTracker::default();
    activity.observe(&engine, 0, true);
    let mut joined = lobby(true);
    joined["payload"]["players"][1]["id"] = json!(42);
    joined["payload"]["players"][1]["name"] = json!("");
    engine.on_event(&joined, 1);
    assert!(activity.observe(&engine, 1, true).is_empty());
    joined["payload"]["players"][1]["name"] = json!("Alice");
    engine.on_event(&joined, 1);
    assert_eq!(
        activity.observe(&engine, 1, true),
        vec![Activity::Joined("Alice".into())]
    );
    joined["payload"]["players"][1]["name"] = json!("");
    engine.on_event(&joined, 1);
    assert!(activity.observe(&engine, 1, true).is_empty());
    joined["payload"]["players"][1]["name"] = json!("Alice");
    engine.on_event(&joined, 2);
    assert!(activity.observe(&engine, 2, true).is_empty());
    engine.on_event(&lobby(false), 3);
    assert_eq!(
        activity.observe(&engine, 3, true),
        vec![Activity::Left("Alice".into())]
    );
}

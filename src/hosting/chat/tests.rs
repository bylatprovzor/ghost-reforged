use super::*;
use crate::hosting::tests::{event, lobby, ready};

fn message(content: &str) -> Value {
    event(
        "ChatMessage",
        json!({"message":{"source":"gameChat","sender":"Player","content":content}}),
    )
}

#[test]
fn simultaneous_commands_are_queued_without_dropping_the_second_reply() {
    let mut replies = Replies::default();
    replies.lobby(Some("Test"));
    replies.push("first", 1000);
    replies.push("second", 1000);
    assert_eq!(replies.pop(1000).unwrap().payload["content"], "first");
    assert!(replies.pop(1999).is_none());
    assert_eq!(replies.pop(2000).unwrap().payload["content"], "second");
    replies.push("old room", 2000);
    replies.lobby(Some("Next"));
    assert!(replies.pop(3000).is_none());
}

#[test]
fn chat_lines_are_utf8_bounded_and_backlogs_expire() {
    let mut replies = Replies::default();
    replies.push(&"é".repeat(700), 0);
    let mut restored = String::new();
    for now in (0..5000).step_by(1000) {
        if let Some(command) = replies.pop(now) {
            let text = command.payload["content"].as_str().unwrap();
            assert!(text.len() <= 240);
            restored.push_str(text);
        }
    }
    assert_eq!(restored, "é".repeat(700));
    replies.push("old", 0);
    assert!(replies.pop(10001).is_none());
}

#[test]
fn informational_commands_respond_during_countdown_but_color_does_not_mutate_it() {
    let mut engine = ready(true);
    engine.on_event(&lobby(true), 1);
    engine.start_now(1);
    let mut chat = Chat::new(None);
    let mut actions = Vec::new();
    for (i, command) in ["?status", "?from", "?map", "?stats"].iter().enumerate() {
        let outcome = chat.process(
            &engine,
            Some(&message(command)),
            true,
            Duration::from_millis(2000 + i as u64 * 400),
            &mut actions,
        );
        assert!(!outcome.start && outcome.color.is_none() && outcome.kick.is_none());
    }
    assert!(actions.iter().any(|c| {
        c.payload["content"]
            .as_str()
            .is_some_and(|t| t.contains("countdown"))
    }));
    assert!(actions.iter().all(|c| c.message == "SendGameChatMessage"));
    chat.process(
        &engine,
        Some(&message("?color blue")),
        true,
        Duration::from_secs(4),
        &mut actions,
    );
    assert!(actions.iter().all(|c| c.message != "SetColor"));
}

#[test]
fn color_requires_a_fresh_matching_player_and_native_ack_before_success() {
    let mut engine = ready(true);
    let mut setup = lobby(true);
    setup["payload"]["players"][1]["color"] = json!(0);
    setup["payload"]["players"][1]["colorChangeEnabled"] = json!(true);
    engine.on_event(&setup, 1);
    let mut chat = Chat::new(None);
    let mut actions = Vec::new();
    chat.process(
        &engine,
        Some(&message("?color blue")),
        true,
        Duration::from_secs(1),
        &mut actions,
    );
    assert!(actions.is_empty());
    chat.process(
        &engine,
        None,
        true,
        Duration::from_millis(1050),
        &mut actions,
    );
    assert_eq!(actions.pop().unwrap().message, "SendGameLobbySetup");
    chat.process(
        &engine,
        Some(&setup),
        true,
        Duration::from_millis(1060),
        &mut actions,
    );
    chat.process(
        &engine,
        None,
        true,
        Duration::from_millis(1110),
        &mut actions,
    );
    assert!(
        actions
            .iter()
            .any(|c| c.message == "SetColor" && c.payload["color"] == 1)
    );
    assert!(!actions.iter().any(|c| c.message == "SendGameChatMessage"));
    setup["payload"]["players"][1]["color"] = json!(1);
    engine.on_event(&setup, 2);
    chat.process(
        &engine,
        Some(&setup),
        true,
        Duration::from_secs(2),
        &mut actions,
    );
    assert!(actions.iter().any(|c| {
        c.payload["content"]
            .as_str()
            .is_some_and(|t| t.contains("color changed to blue"))
    }));
}

#[test]
fn unread_chat_does_not_delay_departure_cancellation_and_replies_survive_the_burst() {
    use crate::{discovery::Endpoint, transport::Client};
    use std::{
        net::{Ipv4Addr, TcpListener},
        sync::mpsc,
        thread,
        time::Instant,
    };
    use tungstenite::Message;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let mut setup = lobby(true);
    let mut second = setup["payload"]["players"][1].clone();
    second["name"] = json!("Second");
    second["slot"] = json!(2);
    setup["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let mut departed = setup.clone();
    departed["payload"]["players"][2]["slotStatus"] = json!(0);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_nodelay(true).unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let began = Instant::now();
        let mut second = message("?map");
        second["payload"]["message"]["sender"] = json!("Second");
        for event in [message("?help"), second, departed] {
            socket
                .send(Message::Text(event.to_string().into()))
                .unwrap();
        }
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        began.elapsed()
    });
    let mut client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "1234567890123".into(),
        version: None,
    })
    .unwrap();
    let mut engine = ready(true);
    engine.on_event(&setup, 1);
    engine.start_now(1);
    let mut chat = Chat::new(None);
    let mut actions = Vec::new();
    tx.send(()).unwrap();
    let began = Instant::now();
    for _ in 0..3 {
        let event = client
            .receive_with_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        actions.extend(super::super::control::dispatch(
            &mut engine,
            Some(&event),
            None,
            true,
            2,
        ));
        chat.process(
            &engine,
            Some(&event),
            true,
            Duration::from_secs(2) + began.elapsed(),
            &mut actions,
        );
    }
    assert!(
        actions.iter().any(|c| c.message == "LobbyCancel"),
        "{actions:?}"
    );
    assert!(actions.iter().any(|c| {
        c.payload["content"]
            .as_str()
            .is_some_and(|t| t.starts_with("Commands:"))
    }));
    assert_eq!(chat.replies.queued.len(), 1);
    tx.send(()).unwrap();
    let measured = server.join().unwrap();
    assert!(
        measured < Duration::from_millis(200),
        "Event processing waited for a poll: {measured:?}"
    );
    println!("Native socket burst: help + map + departure cancellation in {measured:?}");
    chat.process(
        &engine,
        None,
        true,
        Duration::from_millis(2250),
        &mut actions,
    );
    assert!(actions.iter().any(|c| {
        c.payload["content"]
            .as_str()
            .is_some_and(|t| t.starts_with("Map:"))
    }));
}

#[test]
fn unacknowledged_color_expires_without_stopping_the_hosting_engine() {
    let mut engine = ready(true);
    let mut setup = lobby(true);
    setup["payload"]["players"][1]["color"] = json!(0);
    setup["payload"]["players"][1]["colorChangeEnabled"] = json!(true);
    engine.on_event(&setup, 1);
    let mut chat = Chat::new(None);
    let mut actions = Vec::new();
    chat.process(
        &engine,
        Some(&message("?color blue")),
        true,
        Duration::from_secs(1),
        &mut actions,
    );
    engine.on_event(&setup, 6);
    chat.process(
        &engine,
        Some(&setup),
        true,
        Duration::from_secs(6),
        &mut actions,
    );
    assert!(actions.iter().any(|c| {
        c.payload["content"]
            .as_str()
            .is_some_and(|t| t.contains("not applied"))
    }));
    assert!(chat.color.is_none());
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn status_explains_threshold_full_lobby_and_manual_waiting_conditions() {
    let mut engine = ready(true);
    engine.config.auto_start = true;
    engine.config.require_full = false;
    engine.config.min_players = 4;
    assert!(engine.lobby_status(0).contains("1/4 humans; 3 more needed"));
    let mut setup = lobby(true);
    let mut empty = setup["payload"]["players"][1].clone();
    empty["slotStatus"] = json!(0);
    empty["slot"] = json!(2);
    setup["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(empty);
    engine.on_event(&setup, 1);
    engine.config.require_full = true;
    assert!(engine.lobby_status(1).contains("1 open playing slot(s)"));
    engine.config.auto_start = false;
    assert!(engine.lobby_status(1).contains("Automatic start is off"));
    assert!(engine.lobby_status(100).contains("fresh lobby update"));
}

#[test]
fn start_vote_reports_ladder_gate_instead_of_promising_a_countdown() {
    for players in [3, 4] {
        for final_vote in ["?votestart", "?yes", "?yes start"] {
            let mut engine = ready(true);
            let mut setup = lobby(true);
            setup["payload"]["mapData"]["mapName"] = json!(crate::ladder::MAP_NAME);
            for slot in 2..=players {
                let mut player = setup["payload"]["players"][1].clone();
                player["slot"] = json!(slot);
                player["team"] = json!(slot - 1);
                player["name"] = json!(format!("Player{slot}"));
                setup["payload"]["players"]
                    .as_array_mut()
                    .unwrap()
                    .push(player);
            }
            engine.on_event(&setup, 1);
            assert!(!engine.can_start(1));
            let mut chat = Chat::new(None);
            let needed = players / 2 + 1;
            let mut actions = Vec::new();
            for vote in 1..=needed {
                let mut event = message(if vote == needed {
                    final_vote
                } else {
                    "?votestart"
                });
                if vote > 1 {
                    event["payload"]["message"]["sender"] = json!(format!("Player{vote}"));
                }
                let outcome = chat.process(
                    &engine,
                    Some(&event),
                    true,
                    Duration::from_secs(1 + vote),
                    &mut actions,
                );
                assert!(!outcome.start, "blocked vote must not request start");
            }
            let last = actions.last().unwrap().payload["content"].as_str().unwrap();
            assert!(last.contains("start is blocked"), "{last}");
            assert!(!last.contains("Starting the countdown"), "{last}");
            assert!(
                last.contains(if players == 3 {
                    "four human players"
                } else {
                    "ladder preparation"
                }),
                "{last}"
            );
            assert!(actions.iter().all(|c| c.message == "SendGameChatMessage"));
            assert_eq!(engine.phase, Phase::Lobby);

            // An unblocked vote remains a request; native acknowledgement comes later.
            if players == 4 {
                engine.set_ladder_ready(true);
                let outcome = chat.process(
                    &engine,
                    Some(&message(final_vote)),
                    true,
                    Duration::from_secs(5),
                    &mut actions,
                );
                assert!(outcome.start);
                // Replies queue in order; the Russian half of the earlier
                // blocked notice may still be ahead of this one.
                let mut texts = actions.iter().filter_map(|c| c.payload["content"].as_str());
                assert!(texts.any(|t| t.contains("Vote passed. Starting the game") || t.starts_with("Голосование прошло, но")));
                assert_eq!(engine.start_now(5)[0].message, "LobbyStart");
            }
        }
    }
}

#[test]
fn start_vote_still_requests_native_start_on_non_ladder_maps() {
    for final_vote in ["?votestart", "?yes", "?yes start"] {
        let mut engine = ready(true);
        let mut setup = lobby(true);
        let mut player = setup["payload"]["players"][1].clone();
        player["slot"] = json!(2);
        player["name"] = json!("Player2");
        setup["payload"]["players"]
            .as_array_mut()
            .unwrap()
            .push(player);
        engine.on_event(&setup, 1);
        let mut chat = Chat::new(None);
        let mut actions = Vec::new();
        assert!(
            !chat
                .process(
                    &engine,
                    Some(&message("?votestart")),
                    true,
                    Duration::from_secs(1),
                    &mut actions,
                )
                .start
        );
        let mut vote = message(final_vote);
        vote["payload"]["message"]["sender"] = json!("Player2");
        let outcome = chat.process(
            &engine,
            Some(&vote),
            true,
            Duration::from_secs(2),
            &mut actions,
        );
        assert!(outcome.start);
        assert!(
            actions.last().unwrap().payload["content"]
                .as_str()
                .unwrap()
                .contains("Vote passed. Starting the game")
        );
        assert_eq!(engine.start_now(2)[0].message, "LobbyStart");
    }
}

#[test]
fn ladder_wait_reason_counts_a_playing_host_even_when_auto_start_does_not() {
    let mut engine = ready(true);
    engine.config.host_mode = crate::config::HostMode::Player;
    engine.config.count_host = false;
    let mut setup = lobby(true);
    setup["payload"]["mapData"]["mapName"] = json!(crate::ladder::MAP_NAME);
    setup["payload"]["players"][0]["isObserver"] = json!(false);
    setup["payload"]["players"][0]["team"] = json!(3);
    for slot in 2..=3 {
        let mut player = setup["payload"]["players"][1].clone();
        player["slot"] = json!(slot);
        player["team"] = json!(slot - 1);
        player["name"] = json!(format!("Player{slot}"));
        setup["payload"]["players"]
            .as_array_mut()
            .unwrap()
            .push(player);
    }
    engine.on_event(&setup, 1);
    let status = engine.lobby_status(1);
    assert!(
        status.contains("Waiting for ladder preparation to finish"),
        "{status}"
    );
    assert!(
        !status.contains("Waiting for four human players"),
        "{status}"
    );
}

#[test]
fn live_statistics_follow_the_confirmed_game_activity_and_ignore_cancelled_countdowns() {
    let mut engine = ready(true);
    let mut chat = Chat::new(None);
    let mut tracker = super::super::activity::ActivityTracker::default();
    let setup = lobby(true);
    let mut actions = Vec::new();
    chat.process(&engine, Some(&setup), true, Duration::ZERO, &mut actions);
    tracker.observe(&engine, 0, true);
    engine.start_now(1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false,"countdownLocked":false}}),
        ),
        2,
    );
    assert_eq!(engine.phase, Phase::Lobby);
    for activity in tracker.observe(&engine, 2, false) {
        if let crate::hosting::Activity::GameStarted(name) = activity {
            chat.started(&name);
        }
    }
    assert!(chat.history.describe("Player").contains("0 game(s)"));
    engine.on_event(&setup, 3);
    engine.start_now(3);
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        4,
    );
    for activity in tracker.observe(&engine, 4, false) {
        if let crate::hosting::Activity::GameStarted(name) = activity {
            chat.started(&name);
        }
    }
    assert!(chat.history.describe("Player").contains("1 game(s)"));
}

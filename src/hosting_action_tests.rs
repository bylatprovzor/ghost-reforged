use super::*;

#[test]
fn activity_records_countdown_completion_once_without_waiting_for_loading() {
    for (leave, restart) in [(false, false), (true, false), (true, true)] {
        let mut engine = ready(true);
        engine.config.leave_on_load = leave;
        engine.config.restart_on_load = restart;
        let mut tracker = activity::ActivityTracker::default();
        tracker.observe(&engine, 0, true);
        engine.start_now(1);
        for (now, active, locked) in [(2, true, false), (3, true, true), (4, false, false)] {
            engine.on_event(
                &event(
                    "LobbyCountdown",
                    json!({"details":{
                        "countdownActive":active,"countdownLocked":locked
                    }}),
                ),
                now,
            );
            assert_eq!(
                tracker.observe(&engine, now, false),
                if now == 4 {
                    vec![Activity::GameStarted("Test #1".into())]
                } else {
                    vec![]
                }
            );
        }
        engine.on_event(
            &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
            5,
        );
        assert!(tracker.observe(&engine, 5, false).is_empty());
        assert!(tracker.observe(&engine, 6, false).is_empty());
    }
}

#[test]
fn a_cancelled_countdown_is_not_a_started_game_and_the_retry_is_recorded() {
    let mut engine = ready(true);
    engine.config.restart_on_load = true;
    let mut tracker = activity::ActivityTracker::default();
    tracker.observe(&engine, 0, true);
    engine.start_now(1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":false,"countdownLocked":false
            }}),
        ),
        2,
    );
    assert!(tracker.observe(&engine, 2, false).is_empty());
    engine.start_now(3);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":true,"countdownLocked":true
            }}),
        ),
        4,
    );
    assert!(tracker.observe(&engine, 4, false).is_empty());
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":false,"countdownLocked":false
            }}),
        ),
        5,
    );
    assert_eq!(
        tracker.observe(&engine, 5, false),
        vec![Activity::GameStarted("Test #1".into())]
    );
}

fn target_lobby() -> (Engine, Snapshot) {
    let mut engine = ready(true);
    let mut current = lobby(true);
    current["payload"]["players"][1]["id"] = json!(42);
    engine.on_event(&current, 1);
    let snapshot = summarize(engine.owned_lobby(1).unwrap(), true, true);
    (engine, snapshot)
}

#[test]
fn a_manual_kick_requires_a_fresh_matching_player_and_is_sent_only_once() {
    let (engine, snapshot) = target_lobby();
    let mut pending = kick::PendingKick::default();
    pending
        .request(snapshot.kick_target(1).unwrap(), &engine, 1)
        .unwrap();
    assert!(pending.confirm(&engine, 1, true, true, false).is_none());
    let (query, activity) = pending.confirm(&engine, 1, false, true, true).unwrap();
    assert_eq!(query.unwrap().message, "SendGameLobbySetup");
    assert!(activity.is_none());
    assert!(pending.confirm(&engine, 2, true, false, false).is_none());
    let (command, activity) = pending.confirm(&engine, 2, false, true, true).unwrap();
    let command = command.unwrap();
    assert_eq!(command.message, "KickPlayerFromGameLobby");
    assert_eq!(command.payload, json!({"slot":1}));
    assert_eq!(activity.as_deref(), Some("Kick requested · Player"));
    assert!(pending.confirm(&engine, 3, true, true, true).is_none());
}

#[test]
fn queued_old_rosters_cannot_authorize_kicking_a_replacement() {
    for after_query in [false, true] {
        let (mut engine, snapshot) = target_lobby();
        let mut pending = kick::PendingKick::default();
        pending
            .request(snapshot.kick_target(1).unwrap(), &engine, 1)
            .unwrap();
        if after_query {
            let (command, _) = pending.confirm(&engine, 1, false, true, true).unwrap();
            assert_eq!(command.unwrap().message, "SendGameLobbySetup");
        }
        for _ in 0..2 {
            assert!(pending.confirm(&engine, 2, true, true, false).is_none());
        }
        let mut replacement = lobby(true);
        replacement["payload"]["players"][1]["id"] = json!(43);
        engine.on_event(&replacement, 2);
        assert!(
            pending
                .confirm(&engine, 2, true, true, false)
                .unwrap()
                .0
                .is_none()
        );
        assert!(pending.confirm(&engine, 2, false, true, true).is_none());
    }
}

#[test]
fn vote_and_manual_refresh_gate_require_a_drained_queue_on_both_sides() {
    use kick::Refresh::*;
    let mut gate = kick::RefreshGate::default();
    for roster in [true, true, false] {
        assert_eq!(gate.advance(roster, false, true), Wait);
    }
    assert_eq!(gate.advance(false, true, false), Wait);
    assert_eq!(gate.advance(false, true, true), Request);
    assert_eq!(gate.advance(false, true, true), Wait);
    for roster in [true, true, false] {
        assert_eq!(gate.advance(roster, false, true), Wait);
    }
    assert_eq!(gate.advance(false, true, false), Wait);
    assert_eq!(gate.advance(false, true, true), Confirm);
}

#[test]
fn a_passed_vote_stays_cancelled_after_a_join_then_departure_during_refresh() {
    let mut engine = ready(true);
    let mut roster = lobby(true);
    let players = roster["payload"]["players"].as_array_mut().unwrap();
    players.truncate(1);
    for id in 1..=3 {
        players.push(json!({"id":id,"name":format!("Player{id}"),"slot":id,
            "slotStatus":2,"slotType":0,"team":0,"isSelf":false,"isObserver":false,
            "teamChangeEnabled":true}));
    }
    engine.on_event(&roster, 1);
    let mut chat = crate::commands::LobbyCommands::default();
    let settings = crate::commands::CommandSettings::default();
    chat.observe(engine.owned_lobby(1), 1);
    let message = |sender| {
        event(
            "ChatMessage",
            json!({"message":{
        "source":"gameChat","sender":sender,"content":"?votekick Player3"}}),
        )
    };
    chat.handle(&message("Player1"), &settings, 2);
    let target = chat.handle(&message("Player2"), &settings, 3).kick.unwrap();
    let mut pending = Some((target, 3, kick::RefreshGate::default()));
    kick::invalidate_vote(&mut pending, &engine, 3);
    assert!(pending.is_some());
    assert_eq!(
        pending.as_mut().unwrap().2.advance(false, true, true),
        kick::Refresh::Request
    );
    assert_eq!(
        pending.as_mut().unwrap().2.advance(true, false, true),
        kick::Refresh::Wait
    );
    let mut joined = roster.clone();
    let mut fourth = joined["payload"]["players"][1].clone();
    fourth["id"] = json!(4);
    fourth["slot"] = json!(4);
    fourth["name"] = json!("Player4");
    joined["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(fourth);
    engine.on_event(&joined, 4);
    kick::invalidate_vote(&mut pending, &engine, 4);
    engine.on_event(&roster, 4);
    kick::invalidate_vote(&mut pending, &engine, 4);
    assert!(
        pending.is_none(),
        "The original electorate returning must not revive the passed vote"
    );
}

#[test]
fn a_replaced_moved_or_departed_target_is_not_kicked() {
    for (key, value) in [
        ("id", json!(43)),
        ("slot", json!(2)),
        ("name", json!("Replacement")),
        ("slotStatus", json!(0)),
        ("slotType", json!(1)),
        ("isSelf", json!(true)),
    ] {
        let (mut engine, snapshot) = target_lobby();
        let mut pending = kick::PendingKick::default();
        pending
            .request(snapshot.kick_target(1).unwrap(), &engine, 1)
            .unwrap();
        let mut changed = lobby(true);
        changed["payload"]["players"][1]["id"] = json!(42);
        changed["payload"]["players"][1][key] = value;
        engine.on_event(&changed, 2);
        assert!(
            pending
                .confirm(&engine, 2, true, true, false)
                .unwrap()
                .0
                .is_none(),
            "{key}"
        );
    }
}

#[test]
fn kick_controls_exclude_self_computers_empty_slots_and_unknown_identities() {
    let (_, snapshot) = target_lobby();
    assert!(snapshot.kick_target(0).is_none());
    for (status, computer, own, id, name) in [
        (0, false, false, Some(42), "Player"),
        (1, false, false, Some(42), "Player"),
        (2, true, false, Some(42), "Player"),
        (2, false, true, Some(42), "Player"),
        (2, false, false, None, "Player"),
        (2, false, false, Some(255), "Player"),
        (2, false, false, Some(42), ""),
    ] {
        let mut changed = snapshot.clone();
        let player = &mut changed.slots[1];
        player.status = status;
        player.computer = computer;
        player.own = own;
        player.id = id;
        player.name = name.into();
        assert!(changed.kick_target(1).is_none());
    }
    let mut observer = snapshot.clone();
    observer.slots[1].observer = true;
    assert!(
        observer.kick_target(1).is_some(),
        "Other human observers remain removable"
    );
}

#[test]
fn pending_kicks_expire_and_cannot_cross_a_countdown_or_lobby_change() {
    for mode in 0..3 {
        let (mut engine, snapshot) = target_lobby();
        let mut pending = kick::PendingKick::default();
        pending
            .request(snapshot.kick_target(1).unwrap(), &engine, 1)
            .unwrap();
        if mode == 0 {
            engine.start_now(2);
        }
        if mode == 1 {
            let mut other = lobby(true);
            other["payload"]["lobbyName"] = json!("Other lobby");
            engine.on_event(&other, 2);
        }
        assert!(
            pending
                .confirm(&engine, if mode == 2 { 6 } else { 2 }, true, true, false)
                .unwrap()
                .0
                .is_none()
        );
    }
}

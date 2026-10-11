use super::*;

fn ready_start(manual: bool, full: bool) -> (Engine, Value) {
    let mut engine = armed();
    engine.config.host_mode = HostMode::Observer;
    engine.config.require_full = full;
    engine.config.countdown_seconds = 0;
    engine.config.auto_start = !manual;
    let mut players = vec![player(0, true, true, false)];
    for slot in 1..=4 {
        let mut entry = player(slot, false, false, false);
        entry["id"] = json!(slot);
        entry["name"] = json!(format!("Player{slot}"));
        players.push(entry);
    }
    let setup = lobby(players);
    engine.on_event(&setup, 1);
    let commands = if manual {
        engine.start_now(1)
    } else {
        engine.tick(1)
    };
    assert_eq!(commands[0].message, "LobbyStart");
    (engine, setup)
}

fn countdown(active: bool, locked: bool) -> Value {
    event(
        "LobbyCountdown",
        json!({"details":{
            "countdownActive":active,"countdownLocked":locked
        }}),
    )
}

#[test]
fn departure_cancels_even_with_a_closed_replaced_or_still_sufficient_roster() {
    for manual in [false, true] {
        for full in [false, true] {
            for change in 0..5 {
                let (mut engine, mut setup) = ready_start(manual, full);
                engine.on_event(&countdown(true, false), 1);
                let departed = &mut setup["payload"]["players"][4];
                match change {
                    0 => departed["slotStatus"] = json!(0),
                    1 => departed["slotStatus"] = json!(1),
                    2 => departed["slotType"] = json!(1),
                    3 => departed["id"] = json!(99),
                    _ => {
                        departed["team"] = json!(24);
                        departed["isObserver"] = json!(true);
                    }
                }
                let commands = engine.on_event(&setup, 2);
                assert_eq!(
                    commands.iter().map(|c| c.message).collect::<Vec<_>>(),
                    ["LobbyCancel"],
                    "manual={manual}, full={full}, change={change}"
                );
                assert!(
                    matches!(engine.phase, Phase::Starting(_)),
                    "Cancellation needs native acknowledgement"
                );
                assert!(!engine.can_start(2));
                assert!(
                    engine.tick(2).is_empty(),
                    "Do not cancel or start repeatedly"
                );
                engine.on_event(&countdown(false, false), 2);
                assert_eq!(engine.phase, Phase::Lobby);
                assert!(
                    engine.tick(3).is_empty(),
                    "Do not auto-restart with the reduced roster"
                );
            }
        }
    }
}

#[test]
fn observer_departure_does_not_cancel_and_normal_loading_still_completes() {
    let (mut engine, mut setup) = ready_start(false, true);
    setup["payload"]["players"]
        .as_array_mut()
        .unwrap()
        .push(player(5, false, true, false));
    assert!(engine.on_event(&setup, 1).is_empty());
    setup["payload"]["players"].as_array_mut().unwrap().pop();
    for (now, active, locked) in [(2, true, false), (4, true, true), (7, false, false)] {
        assert!(engine.on_event(&setup, now).is_empty());
        assert!(engine.on_event(&countdown(active, locked), now).is_empty());
    }
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        8,
    );
    assert_eq!(engine.phase, Phase::InGame);
}

#[test]
fn late_departure_requests_cancellation_but_does_not_assume_warcraft_accepted_it() {
    let (mut engine, mut setup) = ready_start(false, true);
    engine.on_event(&countdown(true, true), 4);
    setup["payload"]["players"][4]["slotStatus"] = json!(1);
    assert_eq!(engine.on_event(&setup, 5)[0].message, "LobbyCancel");
    assert!(matches!(engine.phase, Phase::Starting(_)));
    engine.on_event(&countdown(false, false), 7);
    assert_eq!(engine.phase, Phase::Lobby);
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        8,
    );
    assert_eq!(
        engine.phase,
        Phase::InGame,
        "A rejected late cancellation must not falsify the game state"
    );
}

#[test]
fn a_cancel_acknowledged_after_lock_does_not_release_the_observer_or_mark_a_game_started() {
    for restart in [false, true] {
        let (mut engine, mut setup) = ready_start(false, true);
        engine.config.leave_on_load = true;
        engine.config.restart_on_load = restart;
        engine.on_event(&countdown(true, true), 4);
        setup["payload"]["players"][4]["slotStatus"] = json!(1);
        assert_eq!(engine.on_event(&setup, 4)[0].message, "LobbyCancel");
        assert!(engine.on_event(&countdown(false, false), 4).is_empty());
        assert_eq!(engine.phase, Phase::Lobby);
        assert!(!engine.game_started());
        assert!(engine.tick(5).is_empty());
    }
}

#[test]
fn resolving_a_profile_during_countdown_does_not_look_like_a_departure() {
    let (mut engine, mut setup) = ready_start(true, true);
    engine.on_event(&countdown(false, false), 1);
    setup["payload"]["players"][4]["name"] = json!("");
    engine.on_event(&setup, 1);
    assert_eq!(engine.start_now(1)[0].message, "LobbyStart");
    setup["payload"]["players"][4]["name"] = json!("Player4");
    assert!(engine.on_event(&setup, 2).is_empty());
    assert!(matches!(engine.phase, Phase::Starting(_)));
    setup["payload"]["players"][4]["id"] = json!(99);
    assert_eq!(engine.on_event(&setup, 3)[0].message, "LobbyCancel");
}

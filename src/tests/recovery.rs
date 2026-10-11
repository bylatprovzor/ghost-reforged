use super::*;

fn populated() -> Value {
    lobby(vec![
        player(0, true, false, false),
        player(1, false, false, false),
        player(2, false, false, false),
    ])
}

#[test]
fn resume_restores_the_rotation_index_for_subsequent_rosters() {
    let mut config = config();
    config.first_number = 2;
    config.resume_lobby = true;
    let mut snapshot = lobby(vec![player(0, true, true, false)]);
    snapshot["payload"]["mapData"]["mapPath"] = json!(r"C:\Maps\next.w3x");
    let mut engine = Engine::new(config);
    assert!(engine.on_event(&snapshot, 0).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
    assert!(engine.on_event(&snapshot, 1).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
    assert_eq!(engine.number(), 1);
}

#[test]
fn native_cancel_before_or_after_departure_rearms_auto_start() {
    for cancel_first in [true, false] {
        let mut engine = armed();
        engine.config.countdown_seconds = 0;
        engine.on_event(&populated(), 1);
        assert_eq!(engine.tick(1)[0].message, "LobbyStart");
        let cancel = event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false,"countdownLocked":false}}),
        );
        let departure = lobby(vec![player(0, true, false, false)]);
        for change in if cancel_first {
            [&cancel, &departure]
        } else {
            [&departure, &cancel]
        } {
            engine.on_event(change, 2);
        }
        assert_eq!(engine.phase, Phase::Lobby);
        assert!(engine.tick(3).is_empty());
        engine.on_event(&populated(), 4);
        assert_eq!(engine.tick(4)[0].message, "LobbyStart");
        assert!(engine.tick(5).is_empty());
    }
}

#[test]
fn native_rejection_with_unchanged_roster_does_not_spin() {
    let mut engine = armed();
    engine.config.countdown_seconds = 0;
    engine.on_event(&populated(), 1);
    engine.tick(1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false}}),
        ),
        2,
    );
    assert_eq!(engine.phase, Phase::Lobby);
    for now in 3..7 {
        assert!(engine.on_event(&populated(), now).is_empty());
        assert!(engine.tick(now).is_empty());
    }
    assert!(engine.on_event(&populated(), 7).is_empty());
    assert_eq!(engine.tick(7)[0].message, "LobbyStart");
}

#[test]
fn full_uses_map_capacity_without_an_unrelated_minimum() {
    let mut engine = armed();
    engine.config.require_full = true;
    engine.config.min_players = 23;
    engine.config.countdown_seconds = 0;
    engine.on_event(&populated(), 1);
    assert_eq!(engine.tick(1)[0].message, "LobbyStart");
}

#[test]
fn results_are_closed_at_startup_and_after_a_missed_start_event() {
    for mut engine in [Engine::new(config()), armed()] {
        let commands =
            engine.on_event(&event("SetGlueScreen", json!({"screen":"SCORE_SCREEN"})), 1);
        assert!(commands.iter().any(|c| c.message == "ScoreScreenClose"));
        assert!(
            commands
                .iter()
                .any(|c| c.message == "InitializeNetProvider")
        );
        assert!(!commands.iter().any(|c| c.message == "LeaveGame"));
    }
}

#[test]
fn loading_an_existing_game_waits_for_results_without_leaving_or_restarting_it() {
    let mut engine = Engine::new(config());
    assert!(
        engine
            .on_event(
                &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
                0
            )
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::WatchingGame);
    assert!(engine.tick(1000).is_empty());
    assert!(
        engine
            .on_event(&event("GameList", json!({})), 1000)
            .is_empty()
    );
    assert_eq!(
        engine.on_event(
            &event("SetGlueScreen", json!({"screen":"SCORE_SCREEN"})),
            1001
        )[0]
        .message,
        "ScoreScreenClose"
    );
}

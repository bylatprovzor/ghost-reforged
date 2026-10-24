use super::*;

#[test]
fn profile_reuse_is_opt_in_persisted_and_applied_to_live_rehost_settings() {
    let profile = crate::profile::Profile::default();
    let mut value = serde_json::to_value(&profile).unwrap();
    value.as_object_mut().unwrap().remove("reuse_session");
    let old: crate::profile::Profile = serde_json::from_value(value.clone()).unwrap();
    assert!(!old.reuse_session);
    value["reuse_session"] = json!(true);
    let enabled: crate::profile::Profile = serde_json::from_value(value).unwrap();
    assert!(enabled.live_settings().reuse_session);
    let saved = serde_json::to_value(&enabled).unwrap();
    assert_eq!(saved["reuse_session"], true);
    let mut engine = Engine::new(config());
    for (rehost, reuse, leave, restart) in [
        (true, true, true, false),
        (true, false, true, true),
        (false, true, false, false),
        (false, false, false, false),
    ] {
        engine.update_rehost_policy(rehost, reuse, 15, 0);
        assert_eq!(engine.config.leave_on_load, leave);
        assert_eq!(engine.config.restart_on_load, restart);
    }
}

#[test]
fn fallback_consumes_a_game_once_and_only_after_process_exit() {
    let mut engine = super::lifecycle::observer_starting();
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        2,
    );
    engine.on_event(&event("LoggedOut", json!({})), 3);
    engine.tick(182);
    assert_eq!(engine.phase, Phase::Restarting);
    assert_eq!(engine.number(), 1);
    engine.client_stopped(183);
    assert_eq!(engine.number(), 2);
    engine.client_stopped(184);
    assert_eq!(engine.number(), 2);
}

#[test]
fn departure_timeout_cannot_restart_a_playing_host_or_an_unowned_client() {
    for observer_config in [false, true] {
        let mut engine = armed();
        engine.config.host_mode = if observer_config {
            HostMode::Observer
        } else {
            HostMode::Player
        };
        engine.config.leave_on_load = true;
        engine.config.countdown_seconds = 0;
        engine.on_event(
            &lobby(vec![
                player(0, true, false, false),
                player(1, false, false, false),
            ]),
            1,
        );
        engine.on_event(
            &event(
                "LobbyCountdown",
                json!({"details":{"countdownActive":true,"countdownLocked":true}}),
            ),
            1,
        );
        engine.on_event(
            &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
            2,
        );
        engine.tick(1000);
        assert_eq!(engine.phase, Phase::InGame);
    }
    let mut unowned = Engine::new(config());
    unowned.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        1,
    );
    unowned.tick(1000);
    assert_eq!(unowned.phase, Phase::WatchingGame);
}

#[test]
fn a_new_native_session_revokes_the_old_departure_timeout() {
    let replacements = [
        event("MultiplayerGameJoinStarted", json!({})),
        event("MultiplayerGameCreateStarted", json!({})),
        event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})),
        lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
        ]),
    ];
    for replacement in replacements {
        let mut engine = super::lifecycle::observer_starting();
        engine.on_event(
            &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
            2,
        );
        engine.on_event(&replacement, 3);
        assert!(engine.session_replaced());
        assert!(engine.shutdown_identity().is_none());
        engine.recover(4);
        assert!(engine.on_event(&event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})), 5).is_empty());
        engine.tick(1000);
        assert!(matches!(engine.phase, Phase::Stopped(_)), "{replacement}");
        assert_eq!(engine.number(), 1);
    }
}

#[test]
fn late_inactive_countdown_does_not_replace_the_departing_game() {
    for active in [json!(false), json!(true), json!(null)] {
        let mut engine = super::lifecycle::observer_starting();
        engine.on_event(&event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})), 2);
        engine.on_event(&event("LobbyCountdown", json!({"details":{"countdownActive":active}})), 3);
        if active == false {
            assert_eq!(engine.phase, Phase::Releasing(2));
            engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 4);
            assert_eq!(engine.number(), 2);
        } else {
            assert!(matches!(engine.phase, Phase::Stopped(_)));
        }
    }
}

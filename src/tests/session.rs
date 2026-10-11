use super::*;

fn online() -> Value {
    event(
        "GameList",
        json!({"games":[{"id":42,"region":"EU","currentPlayers":1,"maxPlayers":4}]}),
    )
}

#[test]
fn sign_in_can_complete_after_a_long_wait_without_repeated_initialization() {
    let mut engine = Engine::new(config());
    assert_eq!(
        engine.on_event(&event("GameList", json!({"games":[]})), 0)[0].message,
        "InitializeNetProvider"
    );
    assert!(engine.tick(31).is_empty());
    assert_eq!(engine.phase, Phase::AwaitSignIn);
    for time in [60, 120, 600] {
        assert!(engine.tick(time).is_empty());
        assert!(
            engine
                .on_event(&event("GameList", json!({"games":[]})), time)
                .is_empty()
        );
    }
    assert_eq!(engine.on_event(&online(), 601)[0].message, "CreateLobby");
    assert!(engine.on_event(&online(), 602).is_empty());
}

#[test]
fn disconnect_revokes_lobby_controls_and_consumes_its_number_once() {
    let mut engine = armed();
    assert!(
        engine
            .on_event(&event("LoggedOut", json!({})), 1)
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::AwaitSignIn);
    assert_eq!(engine.number(), 2);
    assert!(engine.owned_lobby(1).is_none());
    assert!(engine.start_now(1).is_empty());
    assert!(engine.slot_command(0, false, 1).is_none());
    engine.on_event(&event("LoggedOut", json!({})), 2);
    assert_eq!(engine.number(), 2);
    assert!(
        engine.on_event(&online(), 3).is_empty(),
        "A stale listing cannot resume a disconnected lobby"
    );
    engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 4);
    let commands = engine.on_event(&online(), 5);
    assert_eq!(commands[0].payload["gameName"], "GHost Rust #2");
}

#[test]
fn normal_native_countdown_completion_is_not_misread_as_cancellation() {
    let mut engine = armed();
    engine.config.auto_start = false;
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, true),
        ]),
        1,
    );
    engine.start_now(1);
    for (time, active, locked) in [(2, true, false), (5, true, true), (8, false, false)] {
        assert!(
            engine
                .on_event(
                    &event(
                        "LobbyCountdown",
                        json!({"details":{"countdownActive":active,"countdownLocked":locked}})
                    ),
                    time
                )
                .is_empty()
        );
    }
    assert!(matches!(engine.phase, Phase::Starting(_)));
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        8,
    );
    assert_eq!(engine.phase, Phase::InGame);
}

#[test]
fn a_countdown_cancelled_before_lock_returns_to_observation() {
    let mut engine = armed();
    engine.start_now(1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false,"countdownLocked":false}}),
        ),
        2,
    );
    assert_eq!(engine.phase, Phase::Lobby);
    assert!(engine.tick(3).is_empty());
}

#[test]
fn observer_leaves_at_countdown_completion_without_waiting_for_loading() {
    let mut engine = armed();
    engine.config.auto_start = false;
    engine.config.host_mode = HostMode::Observer;
    engine.config.leave_on_load = true;
    engine.on_event(
        &lobby(vec![
            player(0, true, true, false),
            player(1, false, false, true),
        ]),
        1,
    );
    engine.start_now(1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":true,"countdownLocked":true
            }}),
        ),
        4,
    );
    let done = event(
        "LobbyCountdown",
        json!({"details":{
            "countdownActive":false,"countdownLocked":false
        }}),
    );
    assert_eq!(engine.on_event(&done, 7)[0].message, "LeaveGame");
    assert_eq!(engine.phase, Phase::Releasing(7));
    assert!(engine.on_event(&done, 7).is_empty());
    assert!(
        engine
            .on_event(
                &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
                8
            )
            .is_empty()
    );
    let commands = engine.on_event(
        &event("SetGlueScreen", json!({"screen":"SCORE_SCREEN"})),
        10,
    );
    assert_eq!(
        commands.iter().map(|c| c.message).collect::<Vec<_>>(),
        ["ScoreScreenClose", "InitializeNetProvider"]
    );
    assert_eq!(engine.number(), 2);
}

#[test]
fn process_restart_requires_confirmed_loading_and_acknowledged_process_exit() {
    let mut engine = armed();
    engine.config.auto_start = false;
    engine.config.host_mode = HostMode::Observer;
    engine.config.leave_on_load = true;
    engine.config.restart_on_load = true;
    engine.on_event(
        &lobby(vec![
            player(0, true, true, false),
            player(1, false, false, true),
        ]),
        1,
    );
    engine.start_now(1);
    for (time, active, locked) in [(2, true, false), (4, true, true), (7, false, false)] {
        assert!(
            engine
                .on_event(
                    &event(
                        "LobbyCountdown",
                        json!({"details":{
                            "countdownActive":active,"countdownLocked":locked
                        }})
                    ),
                    time
                )
                .is_empty()
        );
        assert!(matches!(engine.phase, Phase::Starting(_)));
    }
    let loading = event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"}));
    assert!(engine.on_event(&loading, 8).is_empty());
    assert_eq!(engine.phase, Phase::Restarting);
    assert_eq!(engine.number(), 1);
    assert!(engine.on_event(&loading, 8).is_empty());
    assert!(engine.on_event(&online(), 8).is_empty());
    engine.client_stopped(8);
    assert_eq!(engine.phase, Phase::Cooldown(23));
    assert_eq!(engine.number(), 2);
    engine.client_stopped(9);
    assert_eq!(engine.number(), 2);
    assert!(engine.on_event(&online(), 22).is_empty());
    engine.tick(23);
    assert_eq!(
        engine.on_event(&online(), 24)[0].payload["gameName"],
        "GHost Rust #2"
    );
}

#[test]
fn restart_never_closes_a_client_whose_host_is_still_a_player() {
    let mut engine = armed();
    engine.config.auto_start = false;
    engine.config.host_mode = HostMode::Observer;
    engine.config.leave_on_load = true;
    engine.config.restart_on_load = true;
    engine.on_event(&lobby(vec![player(0, true, false, false)]), 1);
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":true,"countdownLocked":true
            }}),
        ),
        2,
    );
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        3,
    );
    assert_eq!(engine.phase, Phase::InGame);
}

#[test]
fn a_fresh_client_waits_for_initialization_not_just_provider_selection() {
    let mut engine = Engine::new(config());
    engine.expect_client_initialization();
    assert_eq!(
        engine.on_event(&online(), 0)[0].message,
        "InitializeNetProvider"
    );
    assert!(
        engine
            .on_event(
                &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
                1
            )
            .is_empty()
    );
    engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 2);
    assert!(engine.on_event(&online(), 2).is_empty());
    assert_eq!(engine.phase, Phase::Initializing(0));
    let initialized = event(
        "OnNetProviderInitialized",
        json!({"message":{"isOnline":true}}),
    );
    assert_eq!(engine.on_event(&initialized, 3)[0].message, "CreateLobby");
    assert!(engine.on_event(&initialized, 4).is_empty());
}

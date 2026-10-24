use super::*;

#[test]
fn a_fresh_client_leaves_provider_initialization_to_its_native_menu() {
    let mut engine = Engine::new(config());
    engine.expect_client_initialization();
    assert!(
        engine
            .on_event(&event("GameList", json!({"games":[]})), 0)
            .is_empty()
    );
    engine.on_event(&event("SetGlueScreen", json!({"screen":"LOGIN_DOORS"})), 1);
    let auth = event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}}));
    assert!(engine.on_event(&auth, 2).is_empty());
    engine.recover(3);
    assert!(
        engine
            .on_event(&event("GameList", json!({"games":[]})), 4)
            .is_empty()
    );
    assert!(engine.on_event(&auth, 5).is_empty());
    engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 6);
    assert!(
        engine
            .on_event(&event("GameList", json!({"games":[]})), 7)
            .is_empty()
    );
    let commands = engine.on_event(
        &event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        8,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "CreateLobby");
}

#[test]
fn online_readiness_cannot_overtake_login_screen_completion_or_reconnect() {
    for screen in ["LOGIN_DOORS", "LOGIN"] {
        let mut engine = Engine::new(config());
        engine.expect_client_initialization();
        engine.on_event(&event("GameList", json!({})), 0);
        engine.on_event(&event("SetGlueScreen", json!({"screen":screen})), 1);
        let auth = event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}}));
        let online = event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        );
        engine.on_event(&auth, 2);
        assert!(engine.on_event(&online, 3).is_empty(), "{screen}");
        assert!(
            engine
                .on_event(&event("GameList", json!({"games":[]})), 4)
                .is_empty()
        );
        engine.recover(5);
        engine.on_event(&auth, 6);
        assert!(engine.on_event(&online, 7).is_empty());
        engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 8);
        let commands = engine.on_event(&event("GameList", json!({"games":[]})), 9);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].message, "CreateLobby");
    }
}

#[test]
fn delayed_authentication_retries_provider_initialization_once_and_waits_for_online_proof() {
    let mut engine = Engine::new(config());
    // An attached/recovering client still needs controller initialization.
    engine.on_event(&event("GameList", json!({})), 0);
    let authenticated = event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}}));
    let commands = engine.on_event(&authenticated, 90);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "InitializeNetProvider");
    assert_eq!(engine.phase, Phase::Initializing(90));
    assert!(engine.on_event(&authenticated, 91).is_empty());
    assert!(
        engine
            .on_event(&event("GameList", json!({})), 91)
            .is_empty()
    );
    let commands = engine.on_event(
        &event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        92,
    );
    assert_eq!(commands[0].message, "CreateLobby");
    assert!(engine.on_event(&authenticated, 93).is_empty());
}

#[test]
fn authentication_never_reinitializes_a_lobby_or_a_running_game() {
    let authenticated = event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}}));
    for phase in [
        Phase::Lobby,
        Phase::Countdown(1),
        Phase::Starting(1),
        Phase::InGame,
        Phase::WatchingGame,
    ] {
        let mut engine = Engine::new(config());
        engine.phase = phase.clone();
        assert!(engine.on_event(&authenticated, 2).is_empty());
        assert_eq!(engine.phase, phase);
    }
}

#[test]
fn slow_provider_initialization_never_implies_authentication() {
    let mut engine = Engine::new(config());
    engine.expect_client_initialization();
    engine.on_event(&event("GameList", json!({})), 0);
    for now in [30, 60, 119, 299] {
        assert!(engine.tick(now).is_empty());
        assert_eq!(engine.phase, Phase::Initializing(0));
        assert!(
            engine
                .on_event(&event("GameList", json!({})), now)
                .is_empty()
        );
    }
    assert!(engine.tick(300).is_empty());
    assert_eq!(engine.phase, Phase::AwaitSignIn);
    let commands = engine.on_event(
        &event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        301,
    );
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "CreateLobby");
}

#[test]
fn slow_creation_accepts_native_confirmation_without_duplicate_creation() {
    let mut engine = Engine::new(config());
    engine.on_event(&event("GameList", json!({})), 0);
    engine.on_event(
        &event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        0,
    );
    for now in [30, 37, 60, 119] {
        assert!(engine.tick(now).is_empty());
        assert_eq!(engine.phase, Phase::Creating(0));
    }
    engine.on_event(
        &event(
            "MultiplayerGameCreateResult",
            json!({"details":{"success":true}}),
        ),
        119,
    );
    engine.on_event(&lobby(vec![player(0, true, false, false)]), 119);
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn expired_creation_reconciles_before_retrying() {
    let mut engine = Engine::new(config());
    engine.phase = Phase::Creating(10);
    assert!(engine.tick(129).is_empty());
    let commands = engine.tick(130);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].message, "SendGameLobbySetup");
    assert_eq!(engine.phase, Phase::Recovering(130));
}

#[test]
fn startup_budgets_do_not_extend_start_or_leave_commands() {
    for phase in [Phase::Starting(0), Phase::Leaving(0)] {
        let mut engine = Engine::new(config());
        engine.phase = phase;
        assert_eq!(engine.tick(30)[0].message, "SendGameLobbySetup");
        assert_eq!(engine.phase, Phase::Recovering(30));
    }
}

#[test]
fn older_profiles_gain_safe_budgets_and_invalid_budgets_are_rejected() {
    let mut json = serde_json::to_value(crate::profile::Profile::default()).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("startup_timeout_seconds");
    json.as_object_mut()
        .unwrap()
        .remove("creation_timeout_seconds");
    let mut profile: crate::profile::Profile = serde_json::from_value(json).unwrap();
    assert_eq!(profile.startup_timeout_seconds, 300);
    assert_eq!(profile.creation_timeout_seconds, 120);
    for (startup, creation) in [(59, 120), (901, 120), (300, 29), (300, 301)] {
        profile.startup_timeout_seconds = startup;
        profile.creation_timeout_seconds = creation;
        assert!(profile.validate().is_err());
    }
}

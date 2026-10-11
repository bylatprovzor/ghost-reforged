use super::*;

#[test]
fn incomplete_creation_retry_keeps_its_number_and_game_budget() {
    let mut engine = armed();
    engine.config.max_games = 1;
    let number = engine.number();
    engine.retry_incomplete_lobby();
    assert_eq!(engine.number(), number);
    assert_eq!(engine.phase, Phase::AwaitMenu);
    assert!(engine.owned_lobby(1).is_none());
    let commands = engine.on_event(
        &event(
            "GameList",
            json!({"games":[{
                "id":42,"region":"EU","currentPlayers":1,"maxPlayers":4
            }]}),
        ),
        2,
    );
    assert_eq!(commands[0].message, "CreateLobby");
    assert_eq!(commands[0].payload["gameName"], engine.config.name(number));
    assert_eq!(engine.config.max_games, 1);
}

#[test]
fn an_existing_online_listing_does_not_wait_for_reinitialization() {
    let online = event(
        "GameList",
        json!({"games":[{
            "id":42,"region":"EU","currentPlayers":1,"maxPlayers":4
        }]}),
    );
    for after_request in [false, true] {
        let mut engine = Engine::new(config());
        if after_request {
            engine.on_event(&event("GameList", json!({"games":[]})), 0);
        }
        let commands = engine.on_event(&online, 1);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].message, "CreateLobby");
        assert!(engine.on_event(&online, 2).is_empty());
    }
}

#[test]
fn empty_lan_and_incomplete_listings_do_not_confirm_battle_net() {
    for games in [
        json!([]),
        json!([{"id":1}]),
        json!([{
            "id":1,"region":"LAN","currentPlayers":1,"maxPlayers":4
        }]),
        json!([{"id":0,"region":"EU","currentPlayers":1,"maxPlayers":4}]),
    ] {
        let mut engine = Engine::new(config());
        let listing = event("GameList", json!({"games":games}));
        assert_eq!(
            engine.on_event(&listing, 0)[0].message,
            "InitializeNetProvider"
        );
        assert!(engine.on_event(&listing, 5).is_empty());
        assert_eq!(engine.phase, Phase::Initializing(0));
        engine.tick(30);
        assert_eq!(engine.phase, Phase::AwaitSignIn);
    }
}

#[test]
fn public_and_private_lobbies_use_the_same_host_controller() {
    for private in [false, true] {
        let mut config = config();
        config.private_game = private;
        config.host_mode = HostMode::Observer;
        let mut engine = Engine::new(config);
        engine.on_event(&event("GameList", json!({})), 0);
        let commands = engine.on_event(
            &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
            0,
        );
        assert_eq!(commands[0].payload["privateGame"], private);
        assert_eq!(commands[0].payload["mapSettings"]["settingObservers"], 3);
        engine.on_event(
            &event(
                "MultiplayerGameCreateResult",
                json!({"details":{"success":true}}),
            ),
            1,
        );
        let commands = engine.on_event(&lobby(vec![player(0, true, false, false)]), 1);
        assert_eq!(commands[0].message, "SetTeam");
        assert_eq!(commands[0].payload["team"], 24);
    }
}

fn observer_starting() -> Engine {
    let mut engine = armed();
    engine.config.host_mode = HostMode::Observer;
    engine.config.leave_on_load = true;
    engine.config.countdown_seconds = 0;
    engine.on_event(
        &lobby(vec![
            player(0, true, true, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    assert_eq!(engine.tick(1)[0].message, "LobbyStart");
    engine
}

#[test]
fn loading_release_is_once_and_requires_a_menu_before_rehosting() {
    let mut engine = observer_starting();
    let loading = event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"}));
    assert_eq!(engine.on_event(&loading, 2)[0].message, "LeaveGame");
    assert!(engine.on_event(&loading, 3).is_empty());
    assert!(
        engine
            .on_event(&event("MultiplayerGameLeave", json!({})), 4)
            .is_empty()
    );
    assert!(
        engine
            .on_event(&event("ExitedGame", json!({})), 5)
            .is_empty()
    );
    assert!(engine.on_event(&event("GameList", json!({})), 6).is_empty());
    assert_eq!(engine.phase, Phase::Releasing(2));
    engine.on_event(&event("SetGlueScreen", json!({"screen":"MAIN_MENU"})), 7);
    assert!(matches!(engine.phase, Phase::Cooldown(_)));
    engine.tick(30);
    let next = engine.on_event(&event("GameList", json!({})), 31);
    assert_eq!(next[0].message, "InitializeNetProvider");
    let created = engine.on_event(
        &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        32,
    );
    assert_eq!(created[0].payload["gameName"], "GHost Rust #2");
}

#[test]
fn departure_is_never_sent_for_a_player_host_or_before_loading() {
    let mut engine = armed();
    engine.config.leave_on_load = true;
    assert!(
        engine
            .on_event(
                &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
                2
            )
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::Lobby);
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        3,
    );
    engine.tick(6);
    assert!(
        engine
            .on_event(
                &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
                7
            )
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::InGame);
}

#[test]
fn auto_start_off_waits_and_manual_start_uses_native_countdown() {
    let mut engine = armed();
    engine.config.auto_start = false;
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    assert_eq!(engine.phase, Phase::Lobby);
    assert!(engine.tick(2).is_empty());
    assert_eq!(engine.start_now(2)[0].message, "LobbyStart");
    assert!(engine.start_now(2).is_empty());
}

#[test]
fn manual_start_below_auto_threshold_is_not_immediately_cancelled() {
    let mut engine = armed();
    engine.config.min_players = 20;
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
        ]),
        1,
    );
    assert_eq!(engine.start_now(2)[0].message, "LobbyStart");
    assert!(engine.tick(3).is_empty());
    assert_eq!(engine.phase, Phase::Starting(2));
    assert_eq!(
        engine.on_event(&lobby(vec![player(0, true, false, false)]), 4)[0].message,
        "LobbyCancel"
    );
}

#[test]
fn slot_control_cannot_evict_players_or_use_stale_snapshots() {
    let mut engine = armed();
    let mut open = player(1, false, false, false);
    open["slotStatus"] = json!(0);
    engine.on_event(&lobby(vec![player(0, true, false, false), open]), 1);
    assert!(engine.slot_command(0, false, 2).is_none());
    assert!(engine.slot_command(1, true, 2).is_none());
    assert_eq!(
        engine.slot_command(1, false, 2).unwrap().message,
        "CloseSlot"
    );
    assert!(engine.slot_command(1, false, 30).is_none());
}

#[test]
fn missing_load_exit_acknowledgement_stops_without_creating_another_lobby() {
    let mut engine = observer_starting();
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        2,
    );
    assert!(engine.tick(35).is_empty());
    assert_eq!(engine.phase, Phase::Releasing(2));
    assert!(engine.tick(183).is_empty());
    assert!(matches!(engine.phase, Phase::Stopped(_)));
    assert!(
        engine
            .on_event(&event("GameList", json!({})), 184)
            .is_empty()
    );
}

#[test]
fn slow_loading_exit_closes_score_once_and_rehosts_without_a_menu_callback() {
    let mut engine = observer_starting();
    let loading = event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"}));
    assert_eq!(engine.on_event(&loading, 2)[0].message, "LeaveGame");
    assert!(engine.tick(40).is_empty());
    engine.on_event(&event("ExitedGame", json!({})), 47);
    assert_eq!(engine.phase, Phase::Releasing(2));
    let score = event("SetGlueScreen", json!({"screen":"SCORE_SCREEN"}));
    assert_eq!(engine.on_event(&score, 47)[0].message, "ScoreScreenClose");
    engine.on_event(&event("MultiplayerGameLeave", json!({})), 48);
    assert!(engine.on_event(&score, 48).is_empty());
    assert_eq!(engine.number(), 2);
    engine.tick(62);
    let online = event(
        "GameList",
        json!({"games":[{
            "id":42,"region":"EU","currentPlayers":1,"maxPlayers":4
        }]}),
    );
    assert_eq!(
        engine.on_event(&online, 63)[0].payload["gameName"],
        "GHost Rust #2"
    );
}

#[test]
fn a_manual_native_start_uses_the_same_observer_departure_path() {
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
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{
                "countdownActive":true,"countdownLocked":false
            }}),
        ),
        2,
    );
    assert_eq!(engine.phase, Phase::Starting(2));
    let commands = engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        3,
    );
    assert_eq!(commands[0].message, "LeaveGame");
}

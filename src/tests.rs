use crate::{
    config::{Config, ConfigError, HostMode, Observers, Visibility},
    discovery::Endpoint,
    engine::{Engine, Phase},
    protocol::{self, Lobby},
    transport::Client,
};
use serde_json::{Value, json};
use std::{net::TcpListener, path::PathBuf, thread, time::Duration};

mod automatic_recovery;
mod countdown;
mod lifecycle;
mod recovery;
mod session;

#[test]
fn ranked_lobby_names_use_the_real_counter_and_never_send_an_oversized_name() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("test.w3x");
    std::fs::write(&map, []).unwrap();
    let mut settings = Config {
        maps: vec![map],
        lobby_name: "[RANKED] Survival Chaos #{n}".into(),
        host_mode: HostMode::Player,
        ..Config::default()
    };
    assert!(settings.validate().is_ok());
    let profile = crate::profile::Profile {
        lobby_pattern: settings.lobby_name.clone(),
        ..Default::default()
    };
    assert!(profile.validate().is_ok());
    let mut engine = Engine::new(settings.clone());
    engine.on_event(&event("GameList", json!({})), 0);
    let command = engine.on_event(
        &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        0,
    );
    assert_eq!(command[0].message, "CreateLobby");
    assert_eq!(engine.config.name(1), "[RANKED] Survival Chaos #1");
    settings.first_number = 100_000_000;
    assert!(settings.validate().is_err());
    // Even an unchecked/recovered engine must stop before sending this name.
    let mut engine = Engine::new(settings);
    engine.on_event(&event("GameList", json!({})), 0);
    assert!(
        engine
            .on_event(
                &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
                0
            )
            .is_empty()
    );
    assert!(matches!(engine.phase, Phase::Stopped(_)));
}

fn config() -> Config {
    Config {
        maps: vec![
            PathBuf::from(r"C:\Maps\test.w3x"),
            PathBuf::from(r"C:\Maps\next.w3x"),
        ],
        host_mode: HostMode::Player,
        countdown_seconds: 3,
        ..Config::default()
    }
}
fn event(kind: &str, payload: Value) -> Value {
    json!({"messageType":kind,"payload":payload})
}
fn player(slot: u8, is_self: bool, observer: bool, computer: bool) -> Value {
    json!({"slot":slot,"slotStatus":2,"slotType":if computer {1} else {0},
        "team":if observer {24} else {slot},"isSelf":is_self,"isObserver":observer,"teamChangeEnabled":true})
}
fn lobby(players: Vec<Value>) -> Value {
    event(
        "GameLobbySetup",
        json!({"isHost":true,"lobbyName":"GHost Rust #1",
        "mapData":{"mapPath":r"C:\Maps\test.w3x"},"players":players}),
    )
}
fn armed() -> Engine {
    let mut engine = Engine::new(config());
    assert_eq!(
        engine.on_event(&event("GameList", json!({})), 0)[0].message,
        "InitializeNetProvider"
    );
    assert_eq!(
        engine.on_event(
            &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
            0
        )[0]
        .message,
        "CreateLobby"
    );
    engine.on_event(
        &event(
            "MultiplayerGameCreateResult",
            json!({"details":{"success":true}}),
        ),
        0,
    );
    engine.on_event(&lobby(vec![player(0, true, false, false)]), 0);
    assert_eq!(engine.phase, Phase::Lobby);
    engine
}

#[test]
fn websocket_commands_use_the_installed_clients_webui_envelope() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let message = socket.read().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&message).unwrap(),
            json!({"type":"webui","message":"SendGameLobbySetup","payload":{}})
        );
        socket
            .send(tungstenite::Message::Text(
                event("GameLobbySetup", json!({"isHost":false}))
                    .to_string()
                    .into(),
            ))
            .unwrap();
    });
    let mut client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "1234567890123".into(),
        version: None,
    })
    .unwrap();
    client.send("SendGameLobbySetup", json!({})).unwrap();
    assert_eq!(
        client.receive().unwrap().unwrap()["messageType"],
        "GameLobbySetup"
    );
    server.join().unwrap();
}

#[test]
fn creation_settings_follow_native_dropdown_indexes() {
    // Observed in 3.0.0.24268: input 1 -> output 32 (defeated),
    // input 2 -> output 4194304 (referee), input 3 -> output 16 (full).
    for (observers, index) in [
        (Observers::None, 0),
        (Observers::Defeated, 1),
        (Observers::Referee, 2),
        (Observers::Full, 3),
    ] {
        let mut config = config();
        config.map_settings.observers = observers;
        config.map_settings.visibility = Visibility::Visible;
        let command = protocol::create_lobby(&config, 7, 0);
        assert_eq!(command.payload["mapSettings"]["settingObservers"], index);
        assert_eq!(command.payload["mapSettings"]["settingVisibility"], 3);
        assert_eq!(command.payload["gameName"], "GHost Rust #7");
        assert!(
            !command
                .payload
                .as_object()
                .unwrap()
                .contains_key("password")
        );
    }
}

#[test]
fn a_configured_password_is_preserved() {
    let mut config = config();
    config.lobby_password = "private-test".into();
    assert_eq!(
        protocol::create_lobby(&config, 1, 0).payload["password"],
        "private-test"
    );
}

fn creating() -> Engine {
    let mut engine = Engine::new(config());
    engine.on_event(&event("GameList", json!({})), 0);
    engine.on_event(
        &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        0,
    );
    engine
}

#[test]
fn native_creation_sequence_does_not_require_a_result_callback() {
    let mut engine = creating();
    let setup = lobby(vec![player(0, true, false, false)]);
    engine.on_event(&setup, 1);
    assert_eq!(engine.phase, Phase::Creating(0));
    engine.on_event(&event("MultiplayerGameCreateStarted", json!({})), 2);
    engine.on_event(&setup, 3);
    assert_eq!(engine.phase, Phase::Creating(0));
    engine.on_event(&event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})), 4);
    engine.on_event(&setup, 5);
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn lobby_screen_alone_never_confirms_a_creation() {
    let mut engine = creating();
    engine.on_event(&event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})), 1);
    engine.on_event(&lobby(vec![player(0, true, false, false)]), 2);
    assert_eq!(engine.phase, Phase::Creating(0));
}

#[test]
fn native_creation_still_requires_the_requested_map_and_an_occupied_self() {
    for bad_map in [false, true] {
        let mut engine = creating();
        engine.on_event(&event("MultiplayerGameCreateStarted", json!({})), 1);
        engine.on_event(&event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})), 2);
        let mut setup = lobby(vec![player(0, true, false, false)]);
        if bad_map {
            setup["payload"]["mapData"]["mapPath"] = json!(r"C:\Maps\other.w3x");
        } else {
            setup["payload"]["players"][0]["slotStatus"] = json!(0);
        }
        engine.on_event(&setup, 3);
        assert_eq!(engine.phase, Phase::Creating(0));
        engine.on_event(&lobby(vec![player(0, true, false, false)]), 4);
        assert_eq!(engine.phase, Phase::Lobby);
    }
}

#[test]
fn creation_failure_overrides_native_screen_evidence() {
    for payload in [json!({"details":{"success":false}}), json!({})] {
        let mut engine = creating();
        engine.on_event(&event("MultiplayerGameCreateStarted", json!({})), 1);
        engine.on_event(&event("SetGlueScreen", json!({"screen":"GAME_LOBBY"})), 2);
        engine.on_event(&event("MultiplayerGameCreateResult", payload), 3);
        engine.on_event(&lobby(vec![player(0, true, false, false)]), 4);
        assert!(matches!(engine.phase, Phase::Stopped(_)));
    }
}

#[test]
fn game_paths_strip_windows_extended_prefix_without_losing_unicode() {
    assert_eq!(
        protocol::client_path(std::path::Path::new(r"\\?\C:\Maps\복사본.w3x")),
        "C:/Maps/복사본.w3x"
    );
    assert_eq!(
        protocol::client_path(std::path::Path::new(r"\\?\UNC\server\maps\a.w3x")),
        "//server/maps/a.w3x"
    );
}

#[test]
fn no_commands_before_a_menu_and_bnet_provider_confirmation() {
    let mut engine = Engine::new(config());
    assert!(engine.tick(100).is_empty());
    assert!(
        engine
            .on_event(&event("OnChannelUpdate", json!({})), 100)
            .is_empty()
    );
    engine.on_event(&event("GameList", json!({})), 101);
    assert!(
        engine
            .on_event(
                &event("OnNetProviderChanged", json!({"providerId":"CSDK"})),
                102
            )
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::AwaitSignIn);
}

#[test]
fn both_native_provider_confirmations_create_only_one_lobby() {
    let confirmations = [
        event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
    ];
    for confirmation in &confirmations {
        let mut engine = Engine::new(config());
        let commands = engine.on_event(&event("GameList", json!({})), 0);
        assert_eq!(commands[0].payload, json!({}));
        assert_eq!(engine.on_event(confirmation, 1)[0].message, "CreateLobby");
        for duplicate in &confirmations {
            assert!(engine.on_event(duplicate, 2).is_empty());
        }
        assert_eq!(engine.phase, Phase::Creating(1));
    }
}

#[test]
fn offline_or_malformed_provider_results_never_create() {
    for payload in [
        json!({"message":{"isOnline":false}}),
        json!({"message":{"isOnline":"true"}}),
        json!({"isOnline":true}),
        json!({}),
    ] {
        let mut engine = Engine::new(config());
        engine.on_event(&event("GameList", json!({})), 0);
        let result = event("OnNetProviderInitialized", payload);
        assert_eq!(protocol::bnet_confirmation(&result), Some(false));
        assert!(engine.on_event(&result, 1).is_empty());
        assert_eq!(engine.phase, Phase::AwaitSignIn);
    }
    assert_eq!(
        protocol::bnet_confirmation(&event("GameList", json!({}))),
        None
    );
}

#[test]
fn never_adopts_an_existing_lobby_even_if_its_name_matches() {
    let mut engine = Engine::new(config());
    assert!(
        engine
            .on_event(&lobby(vec![player(0, true, false, false)]), 0)
            .is_empty()
    );
    assert!(matches!(engine.phase, Phase::Stopped(_)));
}

#[test]
fn unconfirmed_empty_native_placeholder_is_not_an_invisible_host() {
    let mut engine = Engine::new(config());
    engine.on_event(&event("GameList", json!({})), 0);
    engine.on_event(
        &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        0,
    );
    assert!(engine.on_event(&lobby(vec![]), 1).is_empty());
    assert_eq!(engine.phase, Phase::Creating(0));
    assert_eq!(engine.tick(31)[0].message, "SendGameLobbySetup");
    assert!(matches!(engine.phase, Phase::Recovering(_)));
}

#[test]
fn player_departure_cancels_wait_without_starting() {
    let mut engine = armed();
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    assert_eq!(engine.phase, Phase::Countdown(4));
    assert!(
        engine
            .on_event(
                &lobby(vec![
                    player(0, true, false, false),
                    player(1, false, false, false)
                ]),
                3
            )
            .is_empty()
    );
    assert!(engine.tick(4).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn stale_snapshot_never_starts_the_game() {
    let mut engine = armed();
    engine.config.countdown_seconds = 10;
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    assert!(engine.tick(11).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn starting_is_cancelled_once_when_a_player_leaves() {
    let mut engine = armed();
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    assert_eq!(engine.tick(4)[0].message, "LobbyStart");
    assert!(engine.tick(5).is_empty());
    assert_eq!(
        engine.on_event(&lobby(vec![player(0, true, false, false)]), 5)[0].message,
        "LobbyCancel"
    );
    assert!(engine.tick(6).is_empty());
}

#[test]
fn host_authority_loss_prevents_later_commands() {
    let mut engine = armed();
    let mut changed = lobby(vec![player(0, true, false, false)]);
    changed["payload"]["isHost"] = json!(false);
    assert!(engine.on_event(&changed, 1).is_empty());
    assert!(matches!(engine.phase, Phase::Stopped(_)));
    assert!(engine.tick(100).is_empty());
}

#[test]
fn spectators_and_computers_do_not_count_as_playing_humans() {
    let native = lobby(vec![
        player(0, true, false, false),
        player(1, false, false, false),
        player(2, false, true, false),
        player(3, false, false, true),
    ]);
    let state: Lobby = serde_json::from_value(native["payload"].clone()).unwrap();
    assert_eq!(state.humans(false), 1);
    assert_eq!(state.humans(true), 2);
    assert!(!state.empty());
}

#[test]
fn an_observer_prevents_empty_lobby_refresh() {
    let mut engine = armed();
    engine.config.empty_rehost_seconds = 2;
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, true, false),
        ]),
        1,
    );
    assert!(engine.tick(4).is_empty());
    assert_eq!(engine.phase, Phase::Lobby);
}

#[test]
fn next_lobby_waits_for_score_screen_cooldown_and_fresh_provider() {
    let mut engine = armed();
    engine.on_event(
        &lobby(vec![
            player(0, true, false, false),
            player(1, false, false, false),
            player(2, false, false, false),
        ]),
        1,
    );
    engine.tick(4);
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        5,
    );
    assert_eq!(engine.phase, Phase::InGame);
    assert!(
        engine
            .on_event(&event("GameList", json!({})), 1000)
            .is_empty()
    );
    engine.on_event(&event("GameClosed", json!({})), 1001);
    assert_eq!(engine.phase, Phase::InGame);
    let score = event("SetGlueScreen", json!({"screen":"SCORE_SCREEN"}));
    assert_eq!(engine.on_event(&score, 1001)[0].message, "ScoreScreenClose");
    assert!(engine.on_event(&score, 1002).is_empty());
    assert!(engine.tick(1015).is_empty());
    assert_eq!(engine.phase, Phase::Cooldown(1016));
    engine.tick(1016);
    assert!(engine.tick(1100).is_empty());
    engine.on_event(&event("GameList", json!({})), 1101);
    let commands = engine.on_event(
        &event("OnNetProviderChanged", json!({"providerId":"BNET"})),
        1102,
    );
    assert_eq!(commands[0].payload["gameName"], "GHost Rust #2");
    assert_eq!(commands[0].payload["filename"], "C:/Maps/next.w3x");
}

#[test]
fn malformed_lobby_schema_stops_the_controller() {
    let mut engine = armed();
    assert!(
        engine
            .on_event(&event("GameLobbySetup", json!({"isHost":true})), 1)
            .is_empty()
    );
    assert!(matches!(engine.phase, Phase::Stopped(_)));
}

#[test]
fn invisible_requirement_does_not_silently_fall_back() {
    assert!(matches!(
        Config::default().ensure_supported(),
        Err(ConfigError::InvisibleUnsupported)
    ));
    let mut engine = Engine::new(Config {
        host_mode: HostMode::Invisible,
        ..config()
    });
    assert!(engine.on_event(&event("GameList", json!({})), 1).is_empty());
    assert!(matches!(engine.phase, Phase::Stopped(_)));
}

#[test]
fn misspelled_settings_are_rejected() {
    assert!(toml::from_str::<Config>("min_player = 5").is_err());
}

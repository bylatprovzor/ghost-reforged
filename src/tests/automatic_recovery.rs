use super::*;

fn roster() -> Value {
    lobby(vec![
        player(0, true, true, false),
        player(1, false, false, false),
        player(2, false, false, false),
    ])
}

fn failed_start() -> Engine {
    let mut engine = armed();
    engine.config.host_mode = HostMode::Observer;
    engine.config.countdown_seconds = 0;
    engine.on_event(&roster(), 1);
    assert_eq!(engine.tick(1)[0].message, "LobbyStart");
    for now in 2..31 {
        engine.on_event(&roster(), now);
    }
    assert_eq!(engine.tick(31)[0].message, "SendGameLobbySetup");
    assert_eq!(engine.phase, Phase::Recovering(31));
    engine
}

#[test]
fn missing_start_ack_refreshes_cancels_and_automatically_retries_the_same_lobby() {
    let mut engine = failed_start();
    assert_eq!(engine.on_event(&roster(), 32)[0].message, "LobbyCancel");
    assert!(engine.on_event(&roster(), 33).is_empty());
    engine.on_event(
        &event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false}}),
        ),
        34,
    );
    assert_eq!(engine.phase, Phase::Lobby);
    for now in 35..39 {
        assert!(engine.on_event(&roster(), now).is_empty());
    }
    engine.on_event(&roster(), 39);
    assert_eq!(engine.tick(39)[0].message, "LobbyStart");
    assert_eq!(engine.number(), 1);
}

#[test]
fn unacknowledged_cancel_recreates_only_a_fresh_matching_lobby_and_waits_for_leave() {
    let mut engine = failed_start();
    engine.on_event(&roster(), 32);
    let mut unrelated = roster();
    unrelated["payload"]["lobbyName"] = json!("another lobby");
    assert!(engine.on_event(&unrelated, 50).is_empty());
    assert!(engine.tick(51).is_empty());
    assert_eq!(engine.on_event(&roster(), 52)[0].message, "LeaveGame");
    assert_eq!(engine.phase, Phase::Leaving(52));
    assert!(
        engine
            .on_event(&event("GameList", json!({"games":[]})), 53)
            .is_empty()
    );
    engine.on_event(&event("MultiplayerGameLeave", json!({})), 54);
    assert!(matches!(engine.phase, Phase::Cooldown(_)));
    assert_eq!(engine.number(), 2);
}

#[test]
fn loading_wins_over_recovery_without_replaying_a_start_or_leaving_a_player_host() {
    let mut engine = failed_start();
    engine.config.host_mode = HostMode::Player;
    assert!(
        engine
            .on_event(
                &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
                32
            )
            .is_empty()
    );
    assert_eq!(engine.phase, Phase::InGame);
    engine.recover(33);
    assert_eq!(engine.phase, Phase::InGame);
}

#[test]
fn a_dead_client_retries_an_unstarted_lobby_but_counts_a_confirmed_game_once() {
    let mut engine = failed_start();
    engine.config.max_games = 1;
    engine.client_lost(32);
    assert_eq!(engine.number(), 1);
    assert_eq!(engine.phase, Phase::AwaitMenu);

    let mut engine = failed_start();
    engine.config.max_games = 1;
    engine.on_event(
        &event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"})),
        32,
    );
    engine.client_lost(33);
    assert_eq!(
        engine.phase,
        Phase::Stopped("Configured game limit reached".into())
    );
}

#[test]
fn loss_of_fresh_rosters_never_sends_a_new_start_or_an_unverified_departure() {
    let mut engine = armed();
    engine.on_event(&roster(), 1);
    engine.start_now(1);
    assert!(engine.tick(9).is_empty());
    assert_eq!(engine.phase, Phase::Starting(1));
    assert_eq!(engine.tick(31)[0].message, "SendGameLobbySetup");
    assert!(matches!(engine.phase, Phase::Recovering(_)));
    assert!(engine.tick(100).is_empty());
    assert!(
        engine
            .on_event(
                &event(
                    "GameList",
                    json!({"games":[{"id":1,"region":"EU","currentPlayers":1,"maxPlayers":4}]})
                ),
                101
            )
            .is_empty()
    );
}

use super::*;

fn event(kind: &str, payload: Value) -> Value {
    json!({"messageType":kind,"payload":payload})
}

fn screen(name: &str) -> Value {
    event("SetGlueScreen", json!({"screen":name}))
}

fn disconnected() -> Session {
    let mut session = Session::default();
    session.observe(
        &event("LoggedOut", json!({})),
        Instant::now() - Duration::from_secs(601),
    );
    session.disconnected_idle_lobby(Some(session.epoch));
    session
}

fn returning() -> Session {
    let mut session = disconnected();
    session.requested = Some(Instant::now() - Duration::from_secs(31));
    session.observe(&screen("LOGIN_DOORS"), Instant::now());
    session
}

#[test]
fn an_idle_owned_lobby_can_recover_after_sign_out_without_extending_the_deadline() {
    let began = Instant::now();
    let budget = Duration::from_secs(600);
    let mut session = Session::default();
    session.disconnected_idle_lobby(Some(session.epoch));
    assert!(!session.due(began + budget, budget));
    session.observe(&event("LoggedOut", json!({})), began);
    assert!(!session.due(began + budget, budget));
    session.disconnected_idle_lobby(Some(session.epoch));
    assert!(!session.due(began + budget - Duration::from_secs(1), budget));
    for callback in [
        event("LoggedOut", json!({})),
        event("GameList", json!({})),
        event("UpdateUserInfo", json!({"user":{"isAuthenticated":false}})),
    ] {
        session.observe(&callback, began + budget);
    }
    assert!(session.due(began + budget, budget));
}

#[test]
fn requesting_sign_in_alone_does_not_authorize_a_restart() {
    let mut session = disconnected();
    let now = Instant::now();
    session.requested = Some(now);
    assert!(!session.due(now + Duration::from_secs(600), Duration::ZERO));
    session.observe(&screen("LOGIN"), now);
    assert!(!session.due(now + Duration::from_secs(29), Duration::ZERO));
    assert!(session.due(now + Duration::from_secs(30), Duration::ZERO));
}

#[test]
fn authentication_gameplay_or_unknown_activity_vetoes_recovery() {
    for callback in [
        event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}})),
        event(
            "OnNetProviderInitialized",
            json!({"message":{"isOnline":true}}),
        ),
        screen("LOADING_SCREEN"),
        screen("SCORE_SCREEN"),
        screen("CAMPAIGN"),
        screen("GAME_LOBBY"),
        screen("UNKNOWN"),
        event("MultiplayerGameCreateStarted", json!({})),
        event(
            "MultiplayerGameCreateResult",
            json!({"details":{"success":true}}),
        ),
        event("MultiplayerGameJoinStarted", json!({})),
        event("MultiplayerGameJoinResult", json!({})),
        event(
            "LobbyCountdown",
            json!({"details":{"countdownActive":false}}),
        ),
        event("GameLobbySetup", json!({})),
        super::super::tests::lobby(false),
        super::super::tests::lobby(true),
    ] {
        let mut session = returning();
        session.observe(&callback, Instant::now());
        session.observe(&screen("LOGIN_DOORS"), Instant::now());
        session.disconnected_idle_lobby(Some(session.epoch));
        assert!(!session.due(Instant::now(), Duration::ZERO), "{callback}");
    }
}

fn server(
    events: Vec<Value>,
    login: bool,
    requests: bool,
) -> (Client, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        if requests {
            for name in ["SendGameLobbySetup", "GetAuthInfo"] {
                let value: Value =
                    serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
                assert_eq!(value["message"], name);
            }
            for value in events {
                socket
                    .send(tungstenite::Message::Text(value.to_string().into()))
                    .unwrap();
            }
        }
        if login {
            let value: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(value["message"], "StartLogin");
        }
        // The caller drops the client after confirmation; no other commands are allowed.
        assert!(socket.read().is_err());
    });
    let client = Client::connect(&crate::discovery::Endpoint {
        pid: 0,
        ports: vec![port],
        session: "test".into(),
        version: None,
    })
    .unwrap();
    (client, worker)
}

#[test]
fn silent_logout_getters_request_login_once_and_do_not_terminate() {
    let (mut client, worker) = server(vec![], true, true);
    let mut session = disconnected();
    let mut pending = VecDeque::new();
    assert!(
        !session
            .confirm(
                &mut client,
                &mut pending,
                &AtomicBool::new(false),
                Duration::ZERO
            )
            .unwrap()
    );
    assert!(session.requested.is_some());
    // The pre-existing session watchdog is already expired. The controller must
    // keep reading this socket so it receives the asynchronous login transition.
    assert!(session.awaiting_reply(Instant::now()));
    assert!(!session.awaiting_reply(Instant::now() + Duration::from_secs(60)));
    assert!(
        !session
            .confirm(
                &mut client,
                &mut pending,
                &AtomicBool::new(false),
                Duration::ZERO
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();
}

#[test]
fn socket_failure_retries_lost_login_responses_without_resetting_the_logout_budget() {
    let mut session = disconnected();
    let signed_out = session.signed_out;
    session.requested = Some(Instant::now());
    session.connection_changed();
    assert_eq!(session.signed_out, signed_out);
    assert!(session.due(Instant::now(), Duration::from_secs(600)));
    assert!(!session.awaiting_reply(Instant::now()));
    session.observe(&screen("LOADING_SCREEN"), Instant::now());
    session.connection_changed();
    assert!(!session.due(Instant::now(), Duration::ZERO));
}

#[test]
fn login_response_keeps_the_socket_until_confirmation_but_new_activity_cancels_the_lease() {
    let mut session = returning();
    assert!(session.awaiting_reply(Instant::now()));
    assert!(session.due(Instant::now(), Duration::ZERO));
    session.observe(&screen("LOADING_SCREEN"), Instant::now());
    assert!(!session.awaiting_reply(Instant::now()));
    let mut session = returning();
    session.observe(
        &event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}})),
        Instant::now(),
    );
    assert!(!session.awaiting_reply(Instant::now()));
}

#[test]
fn a_return_to_login_requires_a_fresh_quiet_confirmation_and_preserves_callbacks() {
    for (events, expected) in [
        (vec![], true),
        (vec![event("GameList", json!({}))], true),
        (
            vec![event(
                "UpdateUserInfo",
                json!({"user":{"isAuthenticated":true}}),
            )],
            false,
        ),
        (vec![super::super::tests::lobby(true)], false),
        (vec![screen("LOADING_SCREEN")], false),
    ] {
        let (mut client, worker) = server(events.clone(), false, true);
        let mut session = returning();
        let mut pending = VecDeque::new();
        assert_eq!(
            session
                .confirm(
                    &mut client,
                    &mut pending,
                    &AtomicBool::new(false),
                    Duration::ZERO
                )
                .unwrap(),
            expected
        );
        assert_eq!(
            pending.into_iter().map(|e| e.value).collect::<Vec<_>>(),
            events
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn queued_activity_and_explicit_stop_prevent_even_a_sign_in_request() {
    for stop in [false, true] {
        let (mut client, worker) = server(vec![], false, false);
        let mut session = disconnected();
        let mut pending = if stop {
            VecDeque::new()
        } else {
            VecDeque::from([screen("LOADING_SCREEN").into()])
        };
        assert!(
            !session
                .confirm(
                    &mut client,
                    &mut pending,
                    &AtomicBool::new(stop),
                    Duration::ZERO
                )
                .unwrap()
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn queued_replay_cannot_reapply_an_old_sign_out_after_authentication() {
    let mut session = disconnected();
    let mut pending: super::super::pending::Callback = event("LoggedOut", json!({})).into();
    session.observe_pending(&mut pending, Instant::now());
    session.observe(
        &event("UpdateUserInfo", json!({"user":{"isAuthenticated":true}})),
        Instant::now(),
    );
    session.observe_pending(&mut pending, Instant::now());
    session.disconnected_idle_lobby(Some(session.epoch));
    assert!(!session.due(Instant::now(), Duration::ZERO));
}

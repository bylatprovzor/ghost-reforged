use super::*;

#[test]
fn handshake_error_preserves_protection_from_received_native_callbacks() {
    for event in [
        json!({"messageType":"MultiplayerGameCreateStarted"}),
        json!({"messageType":"GameLobbySetup","payload":{}}),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket
                .send(tungstenite::Message::Text(event.to_string().into()))
                .unwrap();
            socket.close(None).unwrap();
        });
        let mut client = Client::connect(&discovery::Endpoint {
            pid: 0,
            ports: vec![port],
            session: "fixture".into(),
            version: None,
        })
        .unwrap();
        let mut recovery = super::super::login_recovery::Recovery::default();
        recovery.creation_requested();
        recovery.observe(
            &json!({"messageType":"MultiplayerGameCreateStarted"}),
            Instant::now(),
        );
        recovery.observe(
            &json!({"messageType":"MultiplayerGameLeave"}),
            Instant::now(),
        );
        let mut repair = super::super::repair::Repair::default();
        assert!(
            initial_callbacks(
                &mut client,
                &mut repair,
                &mut recovery,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        let diagnostic = recovery.diagnostic(0).unwrap();
        assert!(diagnostic.contains("eligible=false"), "{diagnostic}");
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn handshake_retries_keep_native_initialization_until_connection_is_delivered() {
    let ready = Ok(discovery::Endpoint {
        pid: 1,
        ports: vec![1],
        session: "test".into(),
        version: None,
    });
    for start in [DiscoveryError::NotRunning, DiscoveryError::NoEndpoint] {
        let mut state = Initialization::default();
        state.observe_discovery(&Err(start));
        // Discovery can succeed on later retries while its first handshake
        // or roster validation fails. No connection reached the engine yet.
        state.observe_discovery(&ready);
        state.observe_discovery(&ready);
        let mut engine = crate::engine::Engine::new(Config::default());
        assert!(
            state.pending,
            "API discovery is not completed native startup"
        );
        if state.connected() {
            engine.expect_client_initialization();
        }
        assert!(
            engine
                .on_event(&json!({"messageType":"GameList","payload":{"games":[]}}), 0)
                .is_empty()
        );
        assert!(engine.on_event(&json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":true}}}), 1).is_empty());
        state.observe_discovery(&ready);
        assert!(
            !state.connected(),
            "The engine already owns the initialization wait"
        );
    }
    let mut attached = Initialization::default();
    attached.observe_discovery(&ready);
    assert!(
        !attached.connected(),
        "An existing client keeps its reconciliation path"
    );
}

#[test]
fn explicit_sign_out_is_bounded_despite_social_traffic_and_duplicate_logouts() {
    let mut wait = SessionWait::default();
    let began = Instant::now();
    let budget = Duration::from_secs(300);
    assert!(wait.check(&Phase::AwaitSignIn, began, budget).is_ok());
    wait.observe(&json!({"messageType":"LoggedOut"}), began);
    for seconds in [100, 200, 299] {
        let now = began + Duration::from_secs(seconds);
        wait.observe(&json!({"messageType":"LoggedOut"}), now);
        wait.observe(&json!({"messageType":"FriendsGetFriends"}), now);
        assert!(wait.check(&Phase::AwaitSignIn, now, budget).is_ok());
    }
    let now = began + budget;
    wait.observe(&json!({"messageType":"GameList"}), now);
    assert!(wait.check(&Phase::AwaitSignIn, now, budget).is_err());
    // A real authentication transition receives the full provider budget.
    wait.observe(
        &json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":true}}}),
        now,
    );
    assert!(wait.check(&Phase::Initializing(300), now, budget).is_ok());
    assert!(
        wait.check(&Phase::Initializing(300), now + budget, budget)
            .is_err()
    );
}

#[test]
fn pending_sign_in_keeps_the_socket_and_late_authentication_gets_a_new_budget() {
    let mut engine = crate::engine::Engine::new(Config::default());
    engine.expect_client_initialization();
    engine.on_event(&json!({"messageType":"GameList","payload":{"games":[]}}), 0);
    let mut wait = SessionWait::default();
    let began = Instant::now();
    let budget = Duration::from_secs(engine.config.startup_timeout_seconds);
    assert!(wait.check(&engine.phase, began, budget).is_ok());
    engine.tick(budget.as_secs());
    assert_eq!(engine.phase, Phase::AwaitSignIn);
    for elapsed in [300, 600, 3600] {
        wait.observe(
            &json!({"messageType":"GameList"}),
            began + Duration::from_secs(elapsed),
        );
        assert!(
            wait.check(&engine.phase, began + Duration::from_secs(elapsed), budget)
                .is_ok()
        );
        assert!(
            engine
                .on_event(
                    &json!({"messageType":"GameList","payload":{"games":[]}}),
                    elapsed
                )
                .is_empty()
        );
    }
    let commands = engine.on_event(
        &json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":true}}}),
        3601,
    );
    assert!(
        commands.is_empty(),
        "The native menu owns fresh-client initialization"
    );
    let authenticated = began + Duration::from_secs(3601);
    assert!(wait.check(&engine.phase, authenticated, budget).is_ok());
    assert!(
        wait.check(&engine.phase, authenticated + budget, budget)
            .is_err()
    );
}

#[test]
fn sign_in_wait_still_recovers_when_the_native_api_stops_answering() {
    let mut wait = SessionWait::default();
    let began = Instant::now();
    let budget = Duration::from_secs(300);
    assert!(wait.check(&Phase::AwaitSignIn, began, budget).is_ok());
    wait.observe(
        &json!({"messageType":"GameList"}),
        began + Duration::from_secs(299),
    );
    assert!(
        wait.check(
            &Phase::AwaitSignIn,
            began + Duration::from_secs(300),
            budget
        )
        .is_ok()
    );
    // WebSocket keepalives do not prove that the native UI is processing commands.
    wait.observe(&Value::Null, began + Duration::from_secs(598));
    assert!(
        wait.check(
            &Phase::AwaitSignIn,
            began + Duration::from_secs(599),
            budget
        )
        .is_err()
    );
}

#[test]
fn recovery_after_hours_of_hosting_gets_a_fresh_session_budget() {
    let began = Instant::now();
    let budget = Duration::from_secs(300);
    let mut wait = SessionWait::default();
    assert!(!wait.expired(&Phase::Initializing(0), began, budget));
    assert!(!wait.expired(&Phase::Lobby, began + Duration::from_secs(20), budget));
    let lost = began + Duration::from_secs(7200);
    assert!(!wait.expired(&Phase::Recovering(7200), lost, budget));
    assert!(!wait.expired(
        &Phase::Recovering(7200),
        lost + Duration::from_secs(299),
        budget
    ));
    assert!(wait.expired(&Phase::Recovering(7200), lost + budget, budget));
}

#[test]
fn late_authentication_gets_a_full_provider_budget_without_callbacks_extending_it() {
    let began = Instant::now();
    let budget = Duration::from_secs(300);
    let mut wait = SessionWait::default();
    assert!(!wait.expired(&Phase::Initializing(0), began, budget));
    let authenticated = began + Duration::from_secs(299);
    assert!(!wait.expired(&Phase::Initializing(299), authenticated, budget));
    assert!(!wait.expired(
        &Phase::Initializing(299),
        authenticated + Duration::from_secs(299),
        budget
    ));
    assert!(wait.expired(&Phase::Initializing(299), authenticated + budget, budget));
}

#[test]
fn startup_keeps_menu_and_placeholder_in_chronological_order_for_repair() {
    let pending = VecDeque::from([
        json!({"messageType":"SetGlueScreen","payload":{"screen":"MAIN_MENU"}}),
        json!({"messageType":"GameLobbySetup","payload":{
            "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,
                "isSelf":false,"isObserver":false,"teamChangeEnabled":false}]
        }}),
    ]);
    let mut pending: VecDeque<super::super::pending::Callback> =
        pending.into_iter().map(Into::into).collect();
    let original = pending.clone();
    prioritize_adoption(&mut pending);
    assert_eq!(pending, original);
}

#[test]
fn a_client_that_exits_before_exposing_its_endpoint_gets_one_launch_request() {
    let mut requested = false;
    assert!(!request_launch(
        &Err(DiscoveryError::NoEndpoint),
        &mut requested
    ));
    assert!(request_launch(
        &Err(DiscoveryError::NotRunning),
        &mut requested
    ));
    assert!(!request_launch(
        &Err(DiscoveryError::NotRunning),
        &mut requested
    ));
    assert!(!request_launch(
        &Err(DiscoveryError::NoEndpoint),
        &mut requested
    ));
}

#[test]
fn adoption_preserves_later_countdown_and_roster_order() {
    let mut pending: VecDeque<super::super::pending::Callback> = [
        json!({"messageType":"GameList"}),
        json!({"messageType":"GameLobbySetup","roster":"A"}),
        json!({"messageType":"LobbyCountdown","active":true}),
        json!({"messageType":"LobbyCountdown","active":false}),
        json!({"messageType":"GameLobbySetup","roster":"B"}),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let expected = [
        pending[1].clone(),
        pending[0].clone(),
        pending[2].clone(),
        pending[3].clone(),
        pending[4].clone(),
    ];
    prioritize_adoption(&mut pending);
    assert_eq!(pending, VecDeque::from(expected));
}

#[test]
fn both_startup_paths_wait_for_native_initialization() {
    assert!(needs_initialization(&Err(DiscoveryError::NotRunning)));
    assert!(needs_initialization(&Err(DiscoveryError::NoEndpoint)));
    assert!(!needs_initialization(&Ok(discovery::Endpoint {
        pid: 1,
        ports: Vec::new(),
        session: String::new(),
        version: None,
    })));
}

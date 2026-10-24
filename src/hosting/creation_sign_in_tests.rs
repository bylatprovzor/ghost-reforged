use super::*;
use serde_json::json;

fn screen(name: &str) -> Value {
    json!({"messageType":"SetGlueScreen","payload":{"screen":name}})
}

fn abandoned() -> Departure {
    let mut state = Departure::default();
    state.requested();
    state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
    state.observe(&json!({"messageType":"LoggedOut"}));
    state.requested_at = Some(Instant::now() - Duration::from_secs(601));
    state.signed_out = Some(Instant::now() - Duration::from_secs(11));
    state
}

fn occupied() -> Value {
    json!({"messageType":"GameLobbySetup","payload":{
        "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
        "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":0,
            "isSelf":false,"isObserver":false,"teamChangeEnabled":true,"name":"Participant"}]}})
}

#[test]
fn sign_in_retry_needs_explicit_loss_a_started_request_and_both_deadlines() {
    let budget = Duration::from_secs(600);
    let now = Instant::now();
    let mut state = abandoned();
    assert!(state.sign_in_due(now, budget));
    state.signed_out = Some(now - Duration::from_secs(9));
    assert!(!state.sign_in_due(now, budget));
    state.signed_out = Some(now - Duration::from_secs(11));
    state.requested_at = Some(now - Duration::from_secs(599));
    assert!(!state.sign_in_due(now, budget));
    state = abandoned();
    state.started = false;
    assert!(!state.sign_in_due(now, budget));
    state = abandoned();
    state.requested = false;
    assert!(!state.sign_in_due(now, budget));
    state = abandoned();
    state.signed_out = None;
    state.observe(
        &json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":false}}}),
    );
    assert!(!state.sign_in_due(now, budget));
    assert!(!state.eligible());
}

#[test]
fn old_login_screens_and_new_authentication_cannot_authorize_restart() {
    let mut state = Departure::default();
    state.requested();
    state.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
    state.observe(&screen("LOGIN_DOORS"));
    state.observe(&json!({"messageType":"LoggedOut"}));
    assert!(!state.eligible());
    state.observe(&screen("LOGIN"));
    assert!(state.eligible());
    for authenticated in [
        json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":true}}}),
        json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":true}}}),
    ] {
        state = abandoned();
        state.observe(&screen("LOGIN_DOORS"));
        state.observe(&authenticated);
        assert!(!state.eligible());
        assert!(!state.sign_in_due(Instant::now(), Duration::ZERO));
        state.observe(&json!({"messageType":"LoggedOut"}));
        assert!(
            !state.eligible(),
            "The earlier login screen cannot cross a new session"
        );
    }
}

#[test]
fn any_usable_unknown_or_game_state_blocks_the_sign_in_retry_permanently() {
    let events = [
        occupied(),
        json!({"messageType":"GameLobbySetup","payload":{}}),
        json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":true}}}),
        json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":"unknown"}}}),
        json!({"messageType":"MultiplayerGameJoinStarted"}),
        json!({"messageType":"MultiplayerGameJoinResult"}),
        json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":false}}}),
        screen("LOADING_SCREEN"),
        screen("SCORE_SCREEN"),
        screen("UNKNOWN_SCREEN"),
    ];
    for event in events {
        for before_login in [true, false] {
            let mut state = abandoned();
            if !before_login {
                state.observe(&screen("LOGIN_DOORS"));
            }
            state.observe(&event);
            state.observe(&screen("LOGIN_DOORS"));
            assert!(!state.eligible(), "{event}");
            assert!(
                !state.sign_in_due(Instant::now(), Duration::ZERO),
                "{event}"
            );
        }
    }
    let mut state = abandoned();
    state.requested();
    state.observe(&screen("LOGIN_DOORS"));
    assert!(!state.eligible());
    assert!(!state.sign_in_due(Instant::now(), Duration::ZERO));
}

fn transport(reply: Option<Value>, login_expected: bool) -> (Client, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(6)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        for expected in ["SendGameLobbySetup", "GetAuthInfo"] {
            let command: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(command["message"], expected);
        }
        if let Some(reply) = reply {
            socket
                .send(tungstenite::Message::Text(reply.to_string().into()))
                .unwrap();
        }
        if login_expected {
            let command: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(command["message"], "StartLogin");
            socket
                .send(tungstenite::Message::Text(
                    screen("LOGIN_DOORS").to_string().into(),
                ))
                .unwrap();
        }
        assert!(
            socket.read().is_err(),
            "No repeated sign-in or destructive command is permitted"
        );
    });
    let client = Client::connect(&crate::discovery::Endpoint {
        pid: 0,
        ports: vec![port],
        session: "fixture".into(),
        version: None,
    })
    .unwrap();
    (client, worker)
}

#[test]
fn wire_retry_waits_for_state_and_sends_sign_in_once_without_authorizing_a_kill() {
    let (mut client, worker) = transport(None, true);
    let mut state = abandoned();
    let mut pending = VecDeque::new();
    let stop = AtomicBool::new(true);
    state
        .request_sign_in(&mut client, &mut pending, &stop, Duration::ZERO)
        .unwrap();
    assert!(!state.login_requested);
    stop.store(false, Ordering::SeqCst);
    state
        .request_sign_in(&mut client, &mut pending, &stop, Duration::ZERO)
        .unwrap();
    assert!(state.login_requested);
    assert!(
        !state.eligible(),
        "Sending StartLogin is not a departure acknowledgement"
    );
    state
        .request_sign_in(&mut client, &mut pending, &stop, Duration::ZERO)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !state.eligible() && Instant::now() < deadline {
        if let Some(event) = client.receive().unwrap() {
            state.observe(&event);
        }
    }
    assert!(state.eligible());
    pending.push_back(occupied().into());
    assert!(!state.confirm(&mut client, &mut pending, &stop).unwrap());
    drop(client);
    worker.join().unwrap();
}

#[test]
fn fresh_roster_or_authentication_during_the_wire_refresh_revokes_sign_in() {
    for reply in [
        occupied(),
        json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":true}}}),
    ] {
        let (mut client, worker) = transport(Some(reply.clone()), false);
        let mut state = abandoned();
        let mut pending = VecDeque::new();
        state
            .request_sign_in(
                &mut client,
                &mut pending,
                &AtomicBool::new(false),
                Duration::ZERO,
            )
            .unwrap();
        assert!(!state.login_requested);
        assert!(!state.eligible());
        assert!(
            pending.iter().any(|event| event.value == reply),
            "Normal dispatch must retain the native callback"
        );
        drop(client);
        worker.join().unwrap();
    }
}


fn empty_departed() -> Departure {
    let mut state = abandoned();
    let players: Vec<Value> = (0..24).map(|slot| json!({
        "slot":slot,"slotStatus":0,"slotType":0,"team":0,
        "isSelf":false,"isObserver":false,"teamChangeEnabled":false,"name":""
    })).collect();
    state.observe(&json!({"messageType":"GameLobbySetup","payload":{
        "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},"players":players
    }}));
    state.observe(&json!({"messageType":"MultiplayerGameLeave"}));
    state
}

#[test]
fn empty_native_departure_retries_explicit_sign_out_without_waiting_startup_budget() {
    let mut state = empty_departed();
    let now = Instant::now();
    state.requested_at = Some(now - Duration::from_secs(60));
    assert!(!state.eligible(), "No native login return yet");
    assert!(state.sign_in_due(now, Duration::from_secs(600)));
    state.signed_out = Some(now - Duration::from_secs(9));
    assert!(!state.sign_in_due(now, Duration::from_secs(600)));
    state.signed_out = Some(now - Duration::from_secs(11));
    state.observe(&screen("LOGIN_DOORS"));
    assert!(state.eligible());
    state.observe(&occupied());
    assert!(!state.eligible());
}

#[test]
fn empty_departure_needs_sign_out_and_fresh_login_even_after_authentication() {
    let mut state = empty_departed();
    state.authenticated();
    assert!(!state.sign_in_due(Instant::now(), Duration::ZERO));
    assert!(!state.eligible());
    state.observe(&screen("LOGIN_DOORS"));
    state.observe(&json!({"messageType":"LoggedOut"}));
    assert!(!state.eligible());
    state.observe(&screen("LOGIN_DOORS"));
    assert!(state.eligible());
    state.authenticated();
    assert!(!state.eligible());
}

#[test]
fn empty_departure_wire_retry_preserves_all_native_revalidation_guards() {
    for reply in [None, Some(occupied())] {
        let expected = reply.is_none();
        let (mut client, worker) = transport(reply, expected);
        let mut state = empty_departed();
        let mut pending = VecDeque::new();
        state.request_sign_in(&mut client, &mut pending, &AtomicBool::new(false), Duration::ZERO).unwrap();
        assert_eq!(state.login_requested, expected);
        assert!(!state.eligible());
        drop(client);
        worker.join().unwrap();
    }
}

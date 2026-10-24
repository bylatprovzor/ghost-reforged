use super::*;

#[test]
fn queued_callback_replay_preserves_recovery_but_a_fresh_second_start_revokes_it() {
    let start = serde_json::json!({"messageType":"MultiplayerGameCreateStarted"});
    let mut recovery = Recovery::default();
    recovery.creation_requested();
    let mut callbacks: VecDeque<super::super::pending::Callback> = [
        start.clone(),
        serde_json::json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}),
        serde_json::json!({"messageType":"MultiplayerGameLeave"}),
    ].into_iter().map(Into::into).collect();
    // Connection handshake inspects its queue before controller dispatch.
    for callback in &mut callbacks {
        recovery.observe_pending(callback, Instant::now());
    }
    assert!(recovery.departure.eligible());
    // Confirmation may inspect it again, and dispatch still retains the marker.
    for callback in &mut callbacks {
        recovery.departure.observe_pending(callback);
    }
    while let Some(mut callback) = callbacks.pop_front() {
        recovery.observe_pending(&mut callback, Instant::now());
    }
    assert!(recovery.departure.eligible());
    // A distinct native occurrence with identical JSON must not be deduplicated.
    let mut second = start.into();
    recovery.observe_pending(&mut second, Instant::now());
    assert!(!recovery.departure.eligible());
}

fn screen(name: &str) -> Value {
    json!({"messageType":"SetGlueScreen","payload":{"screen":name}})
}

fn auth(value: bool) -> Value {
    json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":value}}})
}

fn signed_out() -> Evidence {
    let mut evidence = Evidence::default();
    evidence.observe(
        &json!({"messageType":"LoggedOut"}),
        Instant::now() - Duration::from_secs(600),
    );
    evidence.observe(&screen("LOGIN_DOORS"), Instant::now());
    evidence
}

fn stalled_login() -> Evidence {
    let mut evidence = Evidence::default();
    let began = Instant::now() - Duration::from_secs(601);
    evidence.observe(&auth(false), began);
    evidence.observe(&screen("LOGIN_DOORS"), began);
    evidence
}

#[test]
fn authentication_after_the_login_screen_starts_the_wait_and_success_clears_it() {
    let began = Instant::now();
    let budget = Duration::from_secs(60);
    for success in [
        auth(true),
        json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":true}}}),
    ] {
        let mut evidence = Evidence::default();
        evidence.observe(&screen("LOGIN"), began);
        assert!(!evidence.expired(began + budget, budget));
        evidence.observe(&auth(false), began + budget);
        assert!(!evidence.expired(began + budget + Duration::from_secs(59), budget));
        assert!(evidence.expired(began + budget * 2, budget));
        evidence.observe(&success, began + budget * 2);
        assert!(!evidence.expired(began + budget * 3, budget));
        evidence.observe(&auth(false), began + budget * 3);
        evidence.observe(&screen("LOGIN_DOORS"), began + budget * 4);
        assert!(!evidence.expired(began + budget * 4, budget));
        assert!(evidence.expired(began + budget * 5, budget));
    }
}

#[test]
fn a_late_first_login_screen_gets_its_own_bounded_authentication_wait() {
    let began = Instant::now();
    let budget = Duration::from_secs(600);
    let mut evidence = Evidence::default();
    evidence.observe(&auth(false), began);
    // The failed native boot reached the login screen at 588 seconds.
    let login = began + Duration::from_secs(588);
    evidence.observe(&screen("LOGIN_DOORS"), login);
    assert!(!evidence.expired(began + budget, budget));
    evidence.observe(
        &screen("LOGIN_DOORS"),
        login + budget - Duration::from_secs(1),
    );
    evidence.observe(&auth(false), login + budget - Duration::from_secs(1));
    assert!(!evidence.expired(login + budget - Duration::from_secs(1), budget));
    assert!(evidence.expired(login + budget, budget));
}

#[test]
fn an_explicitly_unauthenticated_unused_login_has_a_bounded_wait() {
    let mut evidence = stalled_login();
    assert!(evidence.expired(Instant::now(), Duration::from_secs(600)));
    evidence.observe(&auth(false), Instant::now());
    evidence.observe(&json!({"messageType":"GameList"}), Instant::now());
    assert!(evidence.expired(Instant::now(), Duration::from_secs(600)));
    evidence.observe(&auth(true), Instant::now());
    assert!(!evidence.expired(Instant::now(), Duration::ZERO));
}

#[test]
fn initial_login_timeout_requires_negative_auth_a_login_screen_and_unused_state() {
    let mut unknown = Evidence::default();
    unknown.observe(&screen("LOGIN_DOORS"), Instant::now());
    assert!(!unknown.expired(Instant::now(), Duration::ZERO));
    for event in [
        screen("MAIN_MENU"),
        screen("LOADING_SCREEN"),
        screen("CAMPAIGN"),
        json!({"messageType":"GameLobbySetup","payload":{}}),
        json!({"messageType":"MultiplayerGameCreateStarted"}),
        json!({"messageType":"MultiplayerGameJoinStarted"}),
    ] {
        let mut evidence = stalled_login();
        evidence.observe(&event, Instant::now());
        assert!(
            !evidence.expired(Instant::now(), Duration::from_secs(600)),
            "{event}"
        );
    }
    let mut recent = Evidence::default();
    recent.observe(&auth(false), Instant::now());
    recent.observe(&screen("LOGIN_DOORS"), Instant::now());
    assert!(!recent.expired(Instant::now(), Duration::from_secs(600)));
}

#[test]
fn departure_diagnostic_reports_the_first_block_once_without_payload_data() {
    let mut recovery = Recovery::default();
    recovery.creation_requested();
    recovery.observe(
        &json!({"messageType":"MultiplayerGameCreateStarted"}),
        Instant::now(),
    );
    recovery.observe(
        &json!({"messageType":"LobbyCountdown","payload":{"details":{
            "countdownActive":false,"private":"secret"
        }}}),
        Instant::now(),
    );
    assert_eq!(
        recovery.diagnostic(0).as_deref(),
        Some(
            "Creation recovery guard: owned=false, departed=false, login_requested=false, returned_to_login=false, eligible=false, blocked_by=a countdown callback"
        )
    );
    recovery.observe(
        &json!({"messageType":"MultiplayerGameLeave"}),
        Instant::now(),
    );
    assert_eq!(
        recovery.diagnostic(0).as_deref(),
        Some(
            "Creation recovery guard: owned=false, departed=true, login_requested=false, returned_to_login=false, eligible=false, blocked_by=a countdown callback"
        )
    );
    recovery.observe(
        &json!({"messageType":"MultiplayerGameJoinStarted"}),
        Instant::now(),
    );
    assert_eq!(recovery.diagnostic(0), None);
    assert!(!recovery.departure.eligible());
}

#[test]
fn creation_roster_diagnostics_retain_only_shape_and_counts() {
    let mut recovery = Recovery::default();
    recovery.creation_requested();
    recovery.observe(
        &json!({"messageType":"MultiplayerGameCreateStarted"}),
        Instant::now(),
    );
    recovery.observe(
        &json!({"messageType":"GameLobbySetup","payload":{
            "isHost":false,"lobbyName":"secret lobby","mapData":{"mapPath":"secret map"},
            "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,"isSelf":false,
                "isObserver":false,"teamChangeEnabled":false,"name":"secret player"}]
        }}),
        Instant::now(),
    );
    let diagnostic = recovery.diagnostic(0).unwrap();
    assert!(diagnostic.contains("blocked_by=a player roster"));
    assert!(diagnostic.contains("slots: 1, occupied: 0, own: 0, computer: 0"));
    assert!(diagnostic.contains("name: true, map: true, empty_slot_placeholder: false"));
    assert!(!diagnostic.contains("secret"));
    assert!(recovery.diagnostic(0).is_none());
}

#[test]
fn reconnect_traffic_does_not_extend_sign_out_and_success_clears_it() {
    let mut evidence = signed_out();
    for event in [
        json!({"messageType":"LoggedOut"}),
        json!({"messageType":"GameList"}),
        auth(false),
    ] {
        evidence.observe(&event, Instant::now());
        assert!(evidence.expired(Instant::now(), Duration::from_secs(600)));
    }
    evidence.observe(&auth(true), Instant::now());
    assert!(!evidence.expired(Instant::now(), Duration::ZERO));
    evidence.observe(&auth(false), Instant::now());
    assert!(!evidence.expired(Instant::now(), Duration::from_secs(600)));
}

#[test]
fn lobby_use_permanently_revokes_startup_reset_even_after_returning_to_login() {
    for event in [
        screen("GAME_LOBBY"),
        screen("LOADING_SCREEN"),
        screen("SCORE_SCREEN"),
        screen("CAMPAIGN"),
        json!({"messageType":"MultiplayerGameCreateStarted"}),
        json!({"messageType":"MultiplayerGameJoinResult"}),
        json!({"messageType":"LobbyCountdown","payload":{"details":{"countdownActive":false}}}),
        json!({"messageType":"GameLobbySetup","payload":{}}),
        json!({"messageType":"GameLobbySetup","payload":{"isHost":false,"lobbyName":"","players":[],"mapData":{"mapPath":"map.w3x"}}}),
    ] {
        let mut evidence = signed_out();
        evidence.observe(&event, Instant::now());
        evidence.observe(&screen("LOGIN_DOORS"), Instant::now());
        assert!(!evidence.expired(Instant::now(), Duration::ZERO), "{event}");
    }
    let mut evidence = signed_out();
    evidence.observe(&screen("CAMPAIGN"), Instant::now());
    assert!(!evidence.expired(Instant::now(), Duration::ZERO));
    let mut recovery = Recovery {
        evidence: signed_out(),
        ..Default::default()
    };
    recovery.creation_requested();
    assert!(!recovery.evidence.expired(Instant::now(), Duration::ZERO));
}

#[test]
fn sign_out_followed_by_a_late_main_menu_can_recover_before_any_lobby_use() {
    let mut evidence = Evidence::default();
    let before = Instant::now() - Duration::from_secs(601);
    evidence.observe(&auth(true), before);
    evidence.observe(&json!({"messageType":"LoggedOut"}), before);
    evidence.observe(&screen("MAIN_MENU"), Instant::now());
    assert!(evidence.expired(Instant::now(), Duration::from_secs(600)));
    evidence.observe(&auth(true), Instant::now());
    assert!(!evidence.expired(Instant::now(), Duration::ZERO));
}

fn server(events: Vec<Value>) -> (Client, std::thread::JoinHandle<()>) {
    server_with_requests(events, 2)
}

fn server_with_requests(
    events: Vec<Value>,
    requests: usize,
) -> (Client, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        for expected in ["SendGameLobbySetup", "GetAuthInfo"]
            .into_iter()
            .take(requests)
        {
            let request: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(request["message"], expected);
        }
        for event in events {
            socket
                .send(tungstenite::Message::Text(event.to_string().into()))
                .unwrap();
        }
        let _ = socket.read();
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
fn fresh_confirmation_preserves_callbacks_and_rejects_new_auth_or_lobby() {
    for (events, expected) in [
        (vec![auth(false)], true),
        (vec![auth(false), auth(true)], false),
        (vec![auth(false), screen("LOADING_SCREEN")], false),
        (
            vec![
                json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":true}}}),
            ],
            false,
        ),
        (
            vec![
                auth(false),
                json!({"messageType":"GameLobbySetup","payload":{}}),
            ],
            false,
        ),
    ] {
        let (mut client, worker) = server(events.clone());
        let mut evidence = signed_out();
        let mut pending = VecDeque::new();
        assert_eq!(
            evidence
                .confirm(
                    &mut client,
                    &mut pending,
                    &AtomicBool::new(false),
                    Duration::from_secs(600)
                )
                .unwrap(),
            expected
        );
        assert_eq!(
            pending
                .into_iter()
                .map(|event| event.value)
                .collect::<Vec<_>>(),
            events
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn attached_client_is_never_claimed_by_login_evidence() {
    for evidence in [signed_out(), stalled_login()] {
        let recovery = Recovery {
            evidence,
            ..Default::default()
        };
        assert!(!recovery.due(std::process::id(), Instant::now(), Duration::ZERO));
    }
}

#[test]
fn session_sign_out_only_arms_a_fresh_verified_empty_lobby() {
    for (phase, occupied, identity, now, expected) in [
        (crate::engine::Phase::Lobby, false, true, 1, true),
        (crate::engine::Phase::Lobby, true, true, 1, false),
        (crate::engine::Phase::Lobby, false, false, 1, false),
        (crate::engine::Phase::Lobby, false, true, 9999, false),
        (crate::engine::Phase::Countdown(1), false, true, 1, false),
        (crate::engine::Phase::Starting(1), false, true, 1, false),
        (crate::engine::Phase::InGame, false, true, 1, false),
        (crate::engine::Phase::WatchingGame, false, true, 1, false),
    ] {
        let mut engine = super::super::tests::ready(occupied);
        engine.phase = phase;
        let mut recovery = Recovery::default();
        recovery.creation_requested();
        recovery.observe(&super::super::tests::lobby(occupied), Instant::now());
        recovery.observe(&json!({"messageType":"LoggedOut"}), Instant::now());
        recovery.disconnected_idle_lobby(&engine, identity, now, Some(0));
        assert_eq!(
            recovery.session.due(Instant::now(), Duration::ZERO),
            expected
        );
        // Even eligible session evidence never claims a client attached by the user.
        assert!(!recovery.due(std::process::id(), Instant::now(), Duration::ZERO));
    }
}

#[test]
fn buffered_logout_from_an_older_empty_lobby_cannot_arm_a_new_occupied_session() {
    let mut recovery = Recovery::default();
    let mut callbacks: VecDeque<super::super::pending::Callback> = [
        json!({"messageType":"LoggedOut"}),
        auth(true),
        super::super::tests::lobby(true),
        json!({"messageType":"LoggedOut"}),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    for callback in &mut callbacks {
        recovery.observe_pending(callback, Instant::now());
    }
    let old_logout = callbacks.front().unwrap().session_epoch;
    let current_logout = callbacks.back().unwrap().session_epoch;
    assert_ne!(old_logout, current_logout);
    // The reconnect handshake has seen B, but the engine is only dispatching A.
    recovery.disconnected_idle_lobby(&super::super::tests::ready(false), true, 1, old_logout);
    assert!(!recovery.session.due(Instant::now(), Duration::ZERO));
    recovery.disconnected_idle_lobby(&super::super::tests::ready(true), true, 1, current_logout);
    assert!(!recovery.session.due(Instant::now(), Duration::ZERO));
    // The same occurrence can arm only when its own verified lobby was empty.
    recovery.disconnected_idle_lobby(&super::super::tests::ready(false), true, 1, current_logout);
    assert!(recovery.session.due(Instant::now(), Duration::ZERO));
}

#[cfg(windows)]
#[test]
fn an_owned_process_can_recover_sign_out_after_a_successful_idle_lobby() {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background_priority::tests::priority_fixture",
            "--nocapture",
        ])
        .env("GHOST_PRIORITY_FIXTURE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let mut startup = crate::background_startup::fixture_claim(child.id());
    let mut recovery = Recovery::default();
    recovery.listener_ready(&mut startup, child.id());
    recovery.creation_requested();
    recovery.observe(&super::super::tests::lobby(false), Instant::now());
    let mut logout = json!({"messageType":"LoggedOut"}).into();
    recovery.observe_pending(&mut logout, Instant::now() - Duration::from_secs(601));
    recovery.disconnected_idle_lobby(
        &super::super::tests::ready(false),
        true,
        1,
        logout.session_epoch,
    );
    assert!(!recovery.evidence.expired(Instant::now(), Duration::ZERO));
    assert!(!recovery.departure.eligible());
    assert!(
        recovery.due(child.id(), Instant::now(), Duration::from_secs(600)),
        "A completed lobby must not permanently prevent signed-out session recovery"
    );
    recovery.listener_ready(&mut startup, child.id());
    assert!(recovery.due(child.id(), Instant::now(), Duration::from_secs(600)));
    assert!(!recovery.due(std::process::id(), Instant::now(), Duration::ZERO));
    let (mut client, worker) = server_with_requests(vec![], 0);
    assert!(
        !recovery
            .confirm(
                child.id(),
                &mut client,
                &mut VecDeque::new(),
                &AtomicBool::new(true),
                Duration::ZERO
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();
    child.stdin.take();
    assert!(child.wait().unwrap().success());
    assert!(!recovery.due(child.id(), Instant::now(), Duration::ZERO));
}

#[test]
fn initial_login_confirmation_rechecks_late_auth_and_lobby_activity() {
    for (events, expected) in [
        (vec![], true),
        (vec![auth(false)], true),
        (vec![auth(true)], false),
        (
            vec![
                json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":true}}}),
            ],
            false,
        ),
        (vec![screen("LOADING_SCREEN")], false),
        (
            vec![json!({"messageType":"GameLobbySetup","payload":{}})],
            false,
        ),
    ] {
        let (mut client, worker) = server(events.clone());
        let mut evidence = stalled_login();
        let mut pending = VecDeque::new();
        assert_eq!(
            evidence
                .confirm(
                    &mut client,
                    &mut pending,
                    &AtomicBool::new(false),
                    Duration::from_secs(600)
                )
                .unwrap(),
            expected
        );
        assert_eq!(
            pending
                .into_iter()
                .map(|event| event.value)
                .collect::<Vec<_>>(),
            events
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn recent_negative_auth_or_social_traffic_cannot_cause_premature_reset() {
    let (mut client, worker) = server_with_requests(vec![], 0);
    let mut pending = VecDeque::from([auth(false).into()]);
    let mut evidence = Evidence::default();
    evidence.observe(&screen("LOGIN_DOORS"), Instant::now());
    evidence.observe(&json!({"messageType":"GameList"}), Instant::now());
    assert!(
        !evidence
            .confirm(
                &mut client,
                &mut pending,
                &AtomicBool::new(false),
                Duration::from_secs(600)
            )
            .unwrap()
    );
    assert_eq!(pending.len(), 1);
    drop(client);
    worker.join().unwrap();
    let (mut client, worker) = server_with_requests(vec![], 0);
    let mut pending = VecDeque::from([auth(true).into()]);
    assert!(
        !signed_out()
            .confirm(
                &mut client,
                &mut pending,
                &AtomicBool::new(false),
                Duration::ZERO
            )
            .unwrap()
    );
    assert_eq!(pending.len(), 1);
    drop(client);
    worker.join().unwrap();
}

#[test]
fn explicit_sign_out_can_recover_when_the_native_getter_does_not_reply() {
    for events in [vec![], vec![json!({"messageType":"GameList"})]] {
        let (mut client, worker) = server(events.clone());
        let mut pending = VecDeque::new();
        assert!(
            signed_out()
                .confirm(
                    &mut client,
                    &mut pending,
                    &AtomicBool::new(false),
                    Duration::from_secs(600)
                )
                .unwrap()
        );
        assert_eq!(
            pending
                .into_iter()
                .map(|event| event.value)
                .collect::<Vec<_>>(),
            events
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[cfg(windows)]
#[test]
fn listener_transfer_preserves_exact_process_ownership_and_stop_prevents_reset() {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background_priority::tests::priority_fixture",
            "--nocapture",
        ])
        .env("GHOST_PRIORITY_FIXTURE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let mut startup = crate::background_startup::fixture_claim(child.id());
    let mut recovery = Recovery::default();
    recovery.listener_ready(&mut startup, child.id());
    assert!(!startup.owns(child.id()));
    recovery.evidence = signed_out();
    assert!(recovery.due(child.id(), Instant::now(), Duration::ZERO));
    assert!(!recovery.due(std::process::id(), Instant::now(), Duration::ZERO));
    recovery.evidence = stalled_login();
    assert!(recovery.due(child.id(), Instant::now(), Duration::from_secs(600)));
    assert!(!recovery.due(std::process::id(), Instant::now(), Duration::from_secs(600)));
    // Socket reconnection must not replace the transferred owner or its deadline.
    recovery.listener_ready(&mut startup, child.id());
    assert!(recovery.due(child.id(), Instant::now(), Duration::ZERO));
    recovery.checked = Some(Instant::now());
    assert!(!recovery.due(child.id(), Instant::now(), Duration::ZERO));
    recovery.checked = None;
    let (mut client, worker) = server_with_requests(vec![], 0);
    assert!(
        !recovery
            .confirm(
                child.id(),
                &mut client,
                &mut VecDeque::new(),
                &AtomicBool::new(true),
                Duration::ZERO
            )
            .unwrap()
    );
    drop(client);
    worker.join().unwrap();
    child.stdin.take();
    assert!(child.wait().unwrap().success());
    assert!(!recovery.due(child.id(), Instant::now(), Duration::ZERO));
}

#[cfg(windows)]
#[test]
fn native_departure_after_an_empty_placeholder_recovers_the_newly_requested_client() {
    native_first_creation_recovery(false, false, false);
}

#[cfg(windows)]
#[test]
fn native_login_return_after_sign_out_recovers_only_the_newly_requested_client() {
    native_first_creation_recovery(true, false, false);
}

#[cfg(windows)]
#[test]
fn native_departure_after_a_host_placeholder_recovers_the_requested_client() {
    native_first_creation_recovery(false, true, false);
}

#[cfg(windows)]
#[test]
fn native_failed_full_empty_creation_recovers_only_the_requested_client() {
    native_first_creation_recovery(true, true, true);
}

#[cfg(windows)]
fn native_first_creation_recovery(
    returned_to_login: bool,
    placeholder_host: bool,
    full_empty: bool,
) {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background_priority::tests::priority_fixture",
            "--nocapture",
        ])
        .env("GHOST_PRIORITY_FIXTURE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let mut startup = crate::background_startup::fixture_claim(child.id());
    let mut recovery = Recovery::default();
    recovery.listener_ready(&mut startup, child.id());
    recovery.creation_requested();
    let mut events = vec![
        json!({"messageType":"MultiplayerGameCreateStarted"}),
        screen("GAME_LOBBY"),
        json!({"messageType":"GameLobbySetup","payload":{
            "isHost":placeholder_host,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":0,"slotType":0,"team":255,
                "isSelf":false,"isObserver":false,"teamChangeEnabled":false,"name":""}]}}),
    ];
    if full_empty {
        events[2]["payload"]["players"] = json!(
            (0..24)
                .map(|slot| json!({
                    "slot":slot,"slotStatus":0,"slotType":0,"team":slot,
                    "isSelf":false,"isObserver":false,"teamChangeEnabled":false,"name":""
                }))
                .collect::<Vec<_>>()
        );
        events.push(json!({"messageType":"MultiplayerGameCreateResult","payload":{"details":{"success":false}}}));
    }
    if returned_to_login {
        events.extend([json!({"messageType":"LoggedOut"}), screen("LOGIN_DOORS")]);
    } else {
        events.push(json!({"messageType":"MultiplayerGameLeave"}));
    }
    for event in events {
        recovery.observe(&event, Instant::now());
    }
    let (mut client, worker) = server_with_requests(vec![], 0);
    assert!(
        !recovery
            .confirm(
                std::process::id(),
                &mut client,
                &mut VecDeque::new(),
                &AtomicBool::new(false),
                Duration::from_secs(600)
            )
            .unwrap()
    );
    assert!(
        !recovery
            .confirm(
                child.id(),
                &mut client,
                &mut VecDeque::new(),
                &AtomicBool::new(true),
                Duration::from_secs(600)
            )
            .unwrap()
    );
    let recovered = recovery
        .confirm(
            child.id(),
            &mut client,
            &mut VecDeque::new(),
            &AtomicBool::new(false),
            Duration::from_secs(600),
        )
        .unwrap();
    drop(client);
    worker.join().unwrap();
    child.stdin.take();
    assert!(child.wait().unwrap().success());
    assert!(
        recovered,
        "A native departure before any usable lobby must not loop on the same failed client"
    );
}

#[test]
fn departure_confirmation_retains_queued_callbacks_and_protects_late_loading() {
    for late in [
        screen("LOADING_SCREEN"),
        json!({"messageType":"GameLobbySetup","payload":{}}),
        json!({"messageType":"GameLobbySetup","payload":{
            "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
            "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,
                "isSelf":true,"isObserver":true,"teamChangeEnabled":true}]}}),
        json!({"messageType":"MultiplayerGameJoinStarted"}),
    ] {
        let mut departure = super::super::creation_departure::Departure::default();
        departure.requested();
        departure.observe(&json!({"messageType":"MultiplayerGameCreateStarted"}));
        departure.observe(&json!({"messageType":"MultiplayerGameLeave"}));
        let (mut client, worker) = server_with_requests(vec![late.clone()], 0);
        let mut pending = VecDeque::from([json!({"messageType":"GameList"}).into()]);
        assert!(
            !departure
                .confirm(&mut client, &mut pending, &AtomicBool::new(false))
                .unwrap()
        );
        assert_eq!(pending.back().map(|event| &event.value), Some(&late));
        drop(client);
        worker.join().unwrap();
    }
}

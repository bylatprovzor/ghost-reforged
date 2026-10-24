use super::*;

#[test]
fn quit_is_durable_even_when_previous_cleanup_failed_and_ipc_failure_is_not_stop() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session::Session::open(dir.path()).unwrap();
    session.last_result = Err("Simulated cleanup failure".into());
    assert!(session.apply(Action::Quit).is_err());
    assert!(session.quitting);
    assert!(intent::quitting(dir.path()).unwrap());
    assert!(!intent::read(dir.path()).unwrap());
    drop(session);
    intent::save(dir.path(), true).unwrap();
    let mut session = session::Session::open(dir.path()).unwrap();
    session.recover();
    assert!(session.quitting);
    assert!(intent::read(dir.path()).unwrap());
    assert!(!intent::quitting(dir.path()).unwrap());
}
use clap::{CommandFactory, Parser};

#[test]
fn cli_schema_and_profile_modes_are_unambiguous() {
    Args::command().debug_assert();
    let args = Args::try_parse_from([
        "ghost",
        "--no-gui",
        "--profile",
        "VPS",
        "--headless",
        "--host",
    ])
    .unwrap();
    assert!(args.headless && args.no_gui && args.host);
    assert!(Args::try_parse_from(["ghost", "--headless", "--normal"]).is_err());
    assert!(
        Args::try_parse_from(["ghost", "--headless", "status"])
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(Args::try_parse_from(["ghost", "--data-dir", "somewhere", "profiles"]).is_ok());
    assert!(Args::try_parse_from(["ghost", "status", "--data-dir", "somewhere"]).is_ok());
}

#[test]
fn selected_profile_roundtrips_headless_and_unknown_or_invalid_fields_do_not_write() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session::Session::open(directory.path()).unwrap();
    assert!(!session.selected().headless);
    session
        .apply(Action::ProfileNew {
            title: "VPS".into(),
        })
        .unwrap();
    assert!(session.selected().minimize_window);
    session.apply(Action::ProfileSet{profile:None,settings:json!({"headless":true,"minimize_window":false,"players":6,"require_full":false,"welcome":{"enabled":false}})}).unwrap();
    let before = std::fs::read(directory.path().join("profiles.json")).unwrap();
    for settings in [
        json!({"players":99}),
        json!({"unknown":true}),
        json!({"auto_start":false}),
    ] {
        assert!(
            session
                .apply(Action::ProfileSet {
                    profile: None,
                    settings
                })
                .is_err()
        );
        assert_eq!(
            std::fs::read(directory.path().join("profiles.json")).unwrap(),
            before
        );
    }
    drop(session);
    let session = session::Session::open(directory.path()).unwrap();
    assert_eq!(session.selected().title, "VPS");
    assert!(session.selected().headless);
    assert!(!session.selected().minimize_window);
    assert_eq!(session.selected().players, 6);
    assert!(!session.selected().welcome.enabled);
}

#[test]
fn selectors_reject_missing_or_ambiguous_titles_and_accept_explicit_indices() {
    let mut profiles = crate::profile::Profiles::default();
    assert_eq!(profile_index(&profiles, "#1").unwrap(), 0);
    assert!(profile_index(&profiles, "#0").is_err());
    assert!(profile_index(&profiles, "missing").is_err());
    profiles.items.push(profiles.items[0].clone());
    assert!(profile_index(&profiles, "Profile 1").is_err());
    assert_eq!(profile_index(&profiles, "#2").unwrap(), 1);
}

#[test]
fn repeated_stop_and_quit_preserve_failed_native_departure() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session::Session::open(directory.path()).unwrap();
    session.last_result = Err("Lobby closure could not be confirmed".into());
    for action in [Action::Stop, Action::Stop, Action::Quit, Action::Quit] {
        intent::save(directory.path(), true).unwrap();
        assert_eq!(
            session.apply(action).unwrap_err(),
            "Lobby closure could not be confirmed"
        );
        assert!(!intent::read(directory.path()).unwrap());
    }
    assert!(session.quitting);
    assert!(session.apply(Action::Status).unwrap()["last_error"].is_string());
}

#[test]
fn health_contains_no_profile_or_lobby_data_and_cannot_start_hosting() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session::Session::open(directory.path()).unwrap();
    let value = session.apply(Action::Health).unwrap();
    let health: Health = serde_json::from_value(value.clone()).unwrap();
    assert!(!health.hosting_requested);
    assert!(health.worker.is_none());
    assert!(value.get("profile").is_none());
    assert!(value.get("lobby").is_none());
    assert!(value.get("client_pid").is_none());
    assert!(!session.running());
}

#[test]
fn stop_remains_effective_when_persistence_fails() {
    let directory = tempfile::tempdir().unwrap();
    intent::save(directory.path(), true).unwrap();
    let mut session = session::Session::open(directory.path()).unwrap();
    let path = directory.path().join("hosting-intent.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(session.apply(Action::Stop).is_err());
    assert_eq!(
        session.apply(Action::Health).unwrap()["hosting_requested"],
        false
    );
}

#[test]
fn non_gui_launch_uses_the_same_profile_store_and_overrides_only_requested_fields() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = session::Session::open(directory.path()).unwrap();
    session
        .apply(Action::ProfileNew {
            title: "VPS".into(),
        })
        .unwrap();
    let args =
        Args::try_parse_from(["ghost", "--profile", "VPS", "--headless", "--no-gui"]).unwrap();
    session.configure(&args).unwrap();
    assert!(session.selected().headless);
    let args = Args::try_parse_from(["ghost", "--normal", "--no-gui"]).unwrap();
    session.configure(&args).unwrap();
    assert!(!session.selected().headless);
    assert!(crate::storage::Store::open(directory.path()).is_err());
    assert!(
        session
            .apply(Action::Slot {
                lobby: "missing".into(),
                number: 1,
                action: "computer".into()
            })
            .is_err()
    );
    assert!(
        session
            .apply(Action::Kick {
                lobby: "missing".into(),
                player: "Somebody#1234".into()
            })
            .is_err()
    );
    assert!(session.apply(Action::Start).is_err());
}

#[cfg(windows)]
#[test]
fn native_same_user_pipe_roundtrips_requests_without_graphical_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let server = ipc::Server::start(directory.path()).unwrap();
    let path = directory.path().to_owned();
    let caller = std::thread::spawn(move || ipc::request(&path, Action::Status));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some((request, reply)) = server.receive().unwrap() {
            assert_eq!(request.version, 1);
            assert!(matches!(request.action, Action::Status));
            reply
                .send(Response {
                    version: 1,
                    result: Ok(json!({"headless":true,"large":"a".repeat(20000)})),
                })
                .unwrap();
            break;
        }
        assert!(Instant::now() < deadline, "No native pipe request");
        std::thread::sleep(Duration::from_millis(10));
    }
    let response = caller.join().unwrap();
    match response {
        Ok(value) => assert_eq!(value["headless"], true),
        Err(error) => panic!("Native control response failed: {error:?}"),
    }
}

use super::*;
use crate::{background_render::process::Process, config::Config, engine::Phase};
use serde_json::json;
use std::{
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, NORMAL_PRIORITY_CLASS,
};

struct Fixture {
    _directory: tempfile::TempDir,
    child: Child,
    game: Background,
    process: Process,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Warcraft III.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let child = Command::new(path)
            .args([
                "--exact",
                "background_priority::tests::priority_fixture",
                "--nocapture",
            ])
            .env("GHOST_PRIORITY_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | NORMAL_PRIORITY_CLASS)
            .spawn()
            .unwrap();
        let game = Background::attach(child.id()).unwrap();
        let process = Process::open_priority(child.id()).unwrap();
        Self {
            _directory: directory,
            child,
            game,
            process,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.child.stdin.take();
        let _ = self.child.wait();
    }
}

#[test]
fn connection_keeps_startup_normal_and_releases_a_previous_lobby_throttle() {
    let mut fixture = Fixture::new();
    connecting(&mut fixture.game).unwrap();
    assert_eq!(fixture.process.priority(), NORMAL_PRIORITY_CLASS);
    fixture.game.background_priority(true).unwrap();
    assert_eq!(fixture.process.priority(), BELOW_NORMAL_PRIORITY_CLASS);
    connecting(&mut fixture.game).unwrap();
    assert_eq!(fixture.process.priority(), NORMAL_PRIORITY_CLASS);
}

#[test]
fn native_priority_requires_a_fresh_confirmed_owned_lobby() {
    let mut fixture = Fixture::new();
    let mut engine = Engine::new(Config {
        headless: true,
        maps: vec!["map.w3x".into()],
        lobby_name: "Test #{n}".into(),
        resume_lobby: true,
        first_number: 2,
        ..Config::default()
    });
    engine.on_event(
        &json!({"messageType":"GameLobbySetup","payload":{
        "isHost":true,"lobbyName":"Test #1","mapData":{"mapPath":"map.w3x"},
        "players":[{"slot":0,"slotStatus":2,"slotType":0,"team":24,"isSelf":true,
            "isObserver":true,"teamChangeEnabled":true}]}}),
        0,
    );
    for phase in [
        Phase::AwaitMenu,
        Phase::Initializing(0),
        Phase::AwaitSignIn,
        Phase::Creating(0),
        Phase::Starting(0),
        Phase::InGame,
        Phase::WatchingGame,
        Phase::Restarting,
        Phase::Releasing(0),
        Phase::Leaving(0),
        Phase::Cooldown(0),
        Phase::Recovering(0),
        Phase::Stopped("test".into()),
    ] {
        engine.phase = phase;
        observe(&mut fixture.game, &engine, true, 0).unwrap();
        assert_eq!(
            fixture.process.priority(),
            NORMAL_PRIORITY_CLASS,
            "{:?}",
            engine.phase
        );
    }
    for phase in [Phase::Lobby, Phase::Countdown(10)] {
        engine.phase = phase;
        for (ready, now, headless, expected) in [
            (true, 0, true, BELOW_NORMAL_PRIORITY_CLASS),
            (false, 0, true, NORMAL_PRIORITY_CLASS),
            (true, 1000, true, NORMAL_PRIORITY_CLASS),
            (true, 0, false, NORMAL_PRIORITY_CLASS),
        ] {
            engine.config.headless = headless;
            observe(&mut fixture.game, &engine, ready, now).unwrap();
            assert_eq!(fixture.process.priority(), expected);
        }
    }
    let mut unowned = Engine::new(Config {
        headless: true,
        ..Config::default()
    });
    unowned.phase = Phase::Lobby;
    observe(&mut fixture.game, &unowned, true, 0).unwrap();
    assert_eq!(fixture.process.priority(), NORMAL_PRIORITY_CLASS);
}

use crate::{background::Background, background_render::process::Process};
use std::{
    io::{BufRead, Read, Write},
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, NORMAL_PRIORITY_CLASS,
};

const FIXTURE: &str = "background_priority::lifecycle_tests::priority_family_fixture";

fn spawn(path: &std::path::Path, role: &str) -> Child {
    Command::new(path)
        .args(["--exact", FIXTURE, "--nocapture", "--test-threads=1"])
        .env("GHOST_PRIORITY_FAMILY_FIXTURE", role)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | NORMAL_PRIORITY_CLASS)
        .spawn()
        .unwrap()
}

#[test]
fn priority_family_fixture() {
    let Some(role) = std::env::var_os("GHOST_PRIORITY_FAMILY_FIXTURE") else {
        return;
    };
    let mut byte = [0u8];
    if role == "browser" {
        let _ = std::io::stdin().read(&mut byte);
        return;
    }
    if std::io::stdin().read(&mut byte).unwrap() == 0 {
        return;
    }
    let browser = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("BlizzardBrowser/BlizzardBrowser.exe");
    let mut child = spawn(&browser, "browser");
    println!("BROWSER {}", child.id());
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().read(&mut byte);
    child.stdin.take();
    assert!(child.wait().unwrap().success());
}

#[test]
fn low_resource_poll_includes_browser_children_created_after_api_connection() {
    let directory = tempfile::tempdir().unwrap();
    let game_path = directory.path().join("Warcraft III.exe");
    let browser_path = directory.path().join("BlizzardBrowser/BlizzardBrowser.exe");
    std::fs::create_dir(browser_path.parent().unwrap()).unwrap();
    for path in [&game_path, &browser_path] {
        std::fs::copy(std::env::current_exe().unwrap(), path).unwrap();
    }
    let mut child = spawn(&game_path, "game");
    let mut background = Background::attach(child.id()).unwrap();
    background.background_priority(true).unwrap();
    child.stdin.as_mut().unwrap().write_all(b"s").unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let browser_pid = loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if let Some((_, pid)) = line.trim().split_once("BROWSER ") {
            break pid.parse::<u32>().unwrap();
        }
    };
    let browser = Process::open_priority(browser_pid).unwrap();
    let initial = browser.priority();
    let began = Instant::now();
    let mut result = Ok(());
    while browser.priority() == NORMAL_PRIORITY_CLASS && began.elapsed() < Duration::from_secs(7) {
        result = background.low_resource(false);
        if result.is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let observed = browser.priority();
    background.background_priority(false).unwrap();
    let restored = browser.priority();
    background.low_resource(false).unwrap();
    let normal_mode = browser.priority();
    drop(background);
    child.stdin.take();
    let exited = child.wait().unwrap().success();
    assert!(exited);
    assert_eq!(initial, NORMAL_PRIORITY_CLASS);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(observed, BELOW_NORMAL_PRIORITY_CLASS);
    assert_eq!(restored, NORMAL_PRIORITY_CLASS);
    assert_eq!(normal_mode, NORMAL_PRIORITY_CLASS);
}

use super::*;
use std::io::BufRead;

#[test]
fn guardian_fixture() {
    let Ok(args) = std::env::var("GHOST_RENDER_GUARDIAN_FIXTURE") else {
        return;
    };
    let args: Vec<_> = args.split_whitespace().map(str::to_owned).collect();
    std::process::exit(if watchdog::run(&args).is_ok() { 0 } else { 1 });
}

#[test]
fn rejects_non_warcraft_target_before_suspending_anything() {
    let me = process::Process::open(std::process::id(), false).unwrap();
    assert!(process::Family::new(me.pid, me.created).is_err());
    assert!(process::Family::new(me.pid, me.created.wrapping_add(1)).is_err());
}

#[test]
fn process_paths_accept_native_windows_separators_and_reject_sibling_installs() {
    use std::path::Path;
    assert!(process::same_path(
        Path::new(r"C:\Game\BlizzardBrowser\BlizzardBrowser.exe"),
        Path::new("c:/game/BlizzardBrowser/BlizzardBrowser.exe")
    ));
    assert!(!process::same_path(
        Path::new(r"C:\Other\BlizzardBrowser\BlizzardBrowser.exe"),
        Path::new("c:/game/BlizzardBrowser/BlizzardBrowser.exe")
    ));
}

fn fixture_command(path: &std::path::Path, test: &str) -> Command {
    let mut command = Command::new(path);
    command
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

fn replies(child: &mut Child) -> Receiver<String> {
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::BufReader::new(output)
            .lines()
            .map_while(Result::ok)
        {
            if let Some(line) = line.strip_prefix("RENDER ")
                && tx.send(line.to_owned()).is_err()
            {
                break;
            }
        }
    });
    rx
}

#[test]
fn echo_fixture() {
    let Ok(mode) = std::env::var("GHOST_RENDER_ECHO") else {
        return;
    };
    let mut browser = if mode != "leaf" {
        let executable = std::env::current_exe().unwrap();
        let path = if mode == "game" {
            executable
                .parent()
                .unwrap()
                .join("BlizzardBrowser/BlizzardBrowser.exe")
        } else {
            executable
        };
        let mut child = fixture_command(&path, "background_render::tests::echo_fixture")
            .env(
                "GHOST_RENDER_ECHO",
                if mode == "game" { "browser" } else { "leaf" },
            )
            .spawn()
            .unwrap();
        let rx = replies(&mut child);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Some((child, rx))
    } else {
        None
    };
    let window = if mode == "game" && std::env::var_os("GHOST_RENDER_WINDOW").is_some() {
        delayed_window()
    } else {
        0
    };
    println!(
        "\nRENDER ready {} {window}",
        browser.as_ref().map_or(0, |b| b.0.id())
    );
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line == "ping" {
            if let Some((child, rx)) = &mut browser {
                writeln!(child.stdin.as_mut().unwrap(), "ping").unwrap();
                rx.recv_timeout(Duration::from_secs(20)).unwrap();
            }
            println!("RENDER pong");
            std::io::stdout().flush().unwrap();
        } else {
            break;
        }
    }
    if let Some((mut child, _)) = browser {
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct Fixture {
    game: Child,
    output: Receiver<String>,
    browser: u32,
    window: usize,
    browser_handle: windows_sys::Win32::Foundation::HANDLE,
    _directory: Option<tempfile::TempDir>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_window(false)
    }
    fn with_window(window: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Warcraft III.exe");
        let browser = directory.path().join("BlizzardBrowser/BlizzardBrowser.exe");
        std::fs::create_dir_all(browser.parent().unwrap()).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        std::fs::copy(&path, &browser).unwrap();
        Self::at(&path, Some(directory), window)
    }
    fn same_installation(&self) -> Self {
        let game = process::Process::open(self.game.id(), false).unwrap();
        Self::at(&game.path, None, false)
    }
    fn at(path: &std::path::Path, directory: Option<tempfile::TempDir>, window: bool) -> Self {
        let mut command = fixture_command(path, "background_render::tests::echo_fixture");
        command.env("GHOST_RENDER_ECHO", "game");
        if window {
            command.env("GHOST_RENDER_WINDOW", "1");
        }
        let mut game = command.spawn().unwrap();
        let output = replies(&mut game);
        let line = output.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut fields = line.strip_prefix("ready ").unwrap().split_whitespace();
        let browser = fields.next().unwrap().parse().unwrap();
        let window = fields.next().unwrap().parse().unwrap();
        let browser_handle = unsafe {
            use windows_sys::Win32::System::Threading::*;
            OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, browser)
        };
        assert!(!browser_handle.is_null());
        Self {
            game,
            output,
            browser,
            window,
            browser_handle,
            _directory: directory,
        }
    }
    fn ping(&mut self) {
        writeln!(self.game.stdin.as_mut().unwrap(), "ping").unwrap();
    }
    fn pong(&self) {
        self.pong_within(Duration::from_secs(7));
    }
    fn pong_within(&self, timeout: Duration) {
        assert_eq!(self.output.recv_timeout(timeout).unwrap(), "pong");
    }
    fn family(&self) -> process::Family {
        let game = process::Process::open(self.game.id(), false).unwrap();
        process::Family::new(game.pid, game.created).unwrap()
    }
}

// This native window deliberately stalls its message pump while its separate
// browser child remains available to prove actual suspension and resumption.
fn delayed_window() -> usize {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let window = CreateWindowExW(
            WS_EX_APPWINDOW,
            windows_sys::w!("STATIC"),
            windows_sys::w!("Warcraft III render transition fixture"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            8,
            8,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        assert!(!window.is_null());
        tx.send(window as usize).unwrap();
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            if message.message == WM_APP + 77 {
                println!("RENDER blocked");
                std::io::stdout().flush().unwrap();
                thread::sleep(LEASE + Duration::from_secs(3));
            } else {
                DispatchMessageW(&message);
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn hidden_only_mode_pauses_and_resumes_without_waiting_for_a_blocked_window() {
    let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::with_window(true);
    let mut background = crate::background::Background::attach(fixture.game.id()).unwrap();
    background.set_minimize_window(false);
    background.hide().unwrap();
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        assert_ne!(PostMessageW(fixture.window as _, WM_APP + 77, 0, 0), 0);
    }
    assert_eq!(
        fixture.output.recv_timeout(Duration::from_secs(2)).unwrap(),
        "blocked"
    );
    background.low_resource(true).unwrap();
    fixture.ping();
    let paused = fixture
        .output
        .recv_timeout(Duration::from_millis(200))
        .is_err();
    let began = Instant::now();
    let resumed = background.low_resource(false);
    let elapsed = began.elapsed();
    if paused {
        fixture.pong();
    }
    let iconic =
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(fixture.window as _) };
    drop(background);
    drop(fixture);
    assert!(paused, "Hidden-only mode must still suspend its browser");
    assert!(resumed.is_ok(), "{resumed:?}");
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    assert_eq!(iconic, 0, "No minimize transition may be requested");
}

#[test]
fn creation_pause_survives_slow_native_lobby_minimization() {
    let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::with_window(true);
    let mut background = crate::background::Background::attach(fixture.game.id()).unwrap();
    background.pause_creation().unwrap();
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        assert_ne!(PostMessageW(fixture.window as _, WM_APP + 77, 0, 0), 0);
    }
    assert_eq!(
        fixture.output.recv_timeout(Duration::from_secs(2)).unwrap(),
        "blocked"
    );
    fixture.ping();
    let until = Instant::now() + LEASE + Duration::from_secs(4);
    let mut released_early = false;
    while Instant::now() < until {
        background.low_resource(true).unwrap();
        if fixture
            .output
            .recv_timeout(Duration::from_millis(100))
            .is_ok()
        {
            released_early = true;
        }
    }
    background.low_resource(false).unwrap();
    if !released_early {
        fixture.pong();
    }
    assert!(
        !released_early,
        "Pending minimization let the existing browser lease expire"
    );
}

#[test]
fn resumed_browser_waits_for_minimization_before_a_new_pause() {
    let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::with_window(true);
    let mut background = crate::background::Background::attach(fixture.game.id()).unwrap();
    background.pause_creation().unwrap();
    background.low_resource(false).unwrap();
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        assert_ne!(PostMessageW(fixture.window as _, WM_APP + 77, 0, 0), 0);
    }
    assert_eq!(
        fixture.output.recv_timeout(Duration::from_secs(2)).unwrap(),
        "blocked"
    );
    background.low_resource(true).unwrap();
    fixture.ping();
    let reply = fixture.output.recv_timeout(Duration::from_millis(500));
    drop(background);
    drop(fixture);
    assert_eq!(
        reply.unwrap(),
        "pong",
        "A resumed guard is not an active creation lease"
    );
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Owned test processes only; no real Warcraft instance is used by these tests.
        let _ = self.game.kill();
        let _ = self.game.wait();
        unsafe {
            use windows_sys::Win32::System::Threading::*;
            TerminateProcess(self.browser_handle, 0);
            WaitForSingleObject(self.browser_handle, 5000);
            windows_sys::Win32::Foundation::CloseHandle(self.browser_handle);
        }
    }
}

#[test]
fn fresh_ancestry_rejects_a_candidate_with_a_stale_parent_hint() {
    let first = Fixture::new();
    let other = first.same_installation();
    let browser = process::Process::open(other.browser, false).unwrap();
    let actual = process::Process::open(other.game.id(), false).unwrap();
    let stale = process::Process::open(first.game.id(), false).unwrap();
    assert!(browser.created >= stale.created);
    assert!(browser.child_of(&actual).unwrap());
    assert!(!browser.child_of(&stale).unwrap());
}

#[test]
fn native_family_pauses_only_its_browser_and_balances_repeated_requests() {
    let mut first = Fixture::new();
    let mut other = first.same_installation();
    let mut family = first.family();
    family.pause().unwrap();
    family.pause().unwrap();
    first.ping();
    assert!(matches!(
        first.output.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    other.ping();
    other.pong();
    family.resume().unwrap();
    family.resume().unwrap();
    first.pong();
    first.ping();
    first.pong();
}

#[test]
fn creation_pause_releases_native_browser_before_departure() {
    let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::new();
    let mut unrelated = fixture.same_installation();
    let mut background = crate::background::Background::attach(fixture.game.id()).unwrap();
    background.pause_creation().unwrap();
    fixture.ping();
    assert!(matches!(
        fixture.output.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    unrelated.ping();
    unrelated.pong();
    background.low_resource(false).unwrap();
    fixture.pong();
}

#[test]
fn native_guardian_resumes_after_dropped_pipe_and_expired_lease() {
    let mut fixture = Fixture::new();
    let mut guard = Guard::start(fixture.game.id()).unwrap();
    guard.set_paused(true).unwrap();
    fixture.ping();
    assert!(
        fixture
            .output
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    // No renewal: the child guardian must resume without help from its owner.
    fixture.pong();
    guard.renewed = Instant::now() - Duration::from_secs(2);
    guard.set_paused(true).unwrap();
    fixture.ping();
    assert!(
        fixture
            .output
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    drop(guard);
    fixture.pong_within(Duration::from_millis(1500));
}

#[test]
fn owner_fixture() {
    let Ok(pid) = std::env::var("GHOST_RENDER_OWNER_FIXTURE") else {
        return;
    };
    let mut guard = Guard::start(pid.parse().unwrap()).unwrap();
    guard.set_paused(true).unwrap();
    println!("\nRENDER paused");
    std::io::stdout().flush().unwrap();
    thread::sleep(Duration::from_secs(12));
}

#[test]
fn native_guardian_resumes_if_its_owner_is_killed() {
    let mut fixture = Fixture::new();
    let mut owner = fixture_command(
        &std::env::current_exe().unwrap(),
        "background_render::tests::owner_fixture",
    )
    .env("GHOST_RENDER_OWNER_FIXTURE", fixture.game.id().to_string())
    .spawn()
    .unwrap();
    let output = replies(&mut owner);
    assert_eq!(
        output.recv_timeout(Duration::from_secs(5)).unwrap(),
        "paused"
    );
    fixture.ping();
    assert!(
        fixture
            .output
            .recv_timeout(Duration::from_millis(200))
            .is_err()
    );
    owner.kill().unwrap();
    owner.wait().unwrap();
    fixture.pong_within(Duration::from_millis(1500));
}

use super::*;
use std::os::windows::process::CommandExt;
use std::{
    io::{BufRead, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const FIXTURE: &str = "GHOST_WINDOW_TEST_FIXTURE";

fn create_window() -> HWND {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "Warcraft III window test\0".encode_utf16().collect();
    let window = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            8,
            8,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        )
    };
    assert!(!window.is_null());
    window
}

#[test]
fn window_fixture() {
    if std::env::var_os(FIXTURE).is_none() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut window = create_window();
    println!("\nWINDOW {}", window as usize);
    std::io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let mut message: MSG = unsafe { zeroed() };
        while unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                DispatchMessageW(&message);
            }
        }
        if let Ok(command) = rx.try_recv() {
            match command.as_str() {
                "show" => unsafe {
                    ShowWindow(window, SW_SHOWNOACTIVATE);
                },
                "launcher" => unsafe {
                    SetWindowTextW(window, windows_sys::w!("Launcher test"));
                },
                "replace" => unsafe {
                    DestroyWindow(window);
                    window = create_window();
                },
                "reset" => unsafe {
                    SetWindowLongPtrW(window, GWL_EXSTYLE, WS_EX_APPWINDOW as isize);
                    ShowWindow(window, SW_SHOWNOACTIVATE);
                },
                "block" => {
                    println!("WINDOW {}", window as usize);
                    std::io::stdout().flush().unwrap();
                    thread::sleep(Duration::from_secs(4));
                    continue;
                }
                "stall" => {
                    println!("WINDOW {}", window as usize);
                    std::io::stdout().flush().unwrap();
                    thread::sleep(Duration::from_secs(10));
                    continue;
                }
                "perpixel" => unsafe {
                    SetWindowLongPtrW(
                        window,
                        GWL_EXSTYLE,
                        (WS_EX_APPWINDOW | WS_EX_LAYERED) as isize,
                    );
                },
                "preview" => unsafe {
                    SetWindowLongPtrW(
                        window,
                        GWL_EXSTYLE,
                        (WS_EX_APPWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT)
                            as isize,
                    );
                    SetLayeredWindowAttributes(window, 0, 0, LWA_ALPHA);
                    ShowWindow(window, SW_SHOWNOACTIVATE);
                },
                "endpreview" => unsafe {
                    ShowWindow(window, SW_HIDE);
                    SetWindowLongPtrW(window, GWL_EXSTYLE, WS_EX_APPWINDOW as isize);
                },
                "quit" => break,
                _ => panic!("Unexpected fixture command"),
            }
            println!("WINDOW {}", window as usize);
            std::io::stdout().flush().unwrap();
        }
        thread::sleep(Duration::from_millis(5));
    }
    unsafe {
        DestroyWindow(window);
    }
}

struct Fixture {
    child: std::process::Child,
    output: std::io::BufReader<std::process::ChildStdout>,
}
impl Fixture {
    fn start() -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "background_window::tests::window_fixture",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(FIXTURE, "1")
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = std::io::BufReader::new(child.stdout.take().unwrap());
        Self { child, output }
    }
    fn window(&mut self) -> HWND {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "Fixture exited"
            );
            if let Some(handle) = line.trim().strip_prefix("WINDOW ") {
                return handle.parse::<usize>().unwrap() as HWND;
            }
        }
    }
    fn command(&mut self, command: &str) -> HWND {
        writeln!(self.child.stdin.as_mut().unwrap(), "{command}").unwrap();
        self.window()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_transparent(window: HWND) -> bool {
    (unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_LAYERED as isize != 0 })
        && layered_attributes(window)
            .is_some_and(|(_, alpha, flags)| alpha == 0 && flags & LWA_ALPHA != 0)
}

fn eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Window protection did not converge"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn low_resource_mode_minimizes_and_restores_without_revealing_the_window() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let guard = Guard::start(fixture.child.id()).unwrap();
    guard.minimize(true).unwrap();
    eventually(|| unsafe { IsIconic(window) != 0 && IsWindowVisible(window) == 0 });
    assert!(is_transparent(window));
    fixture.command("reset");
    eventually(|| unsafe { IsIconic(window) != 0 && IsWindowVisible(window) == 0 });
    guard.minimize(false).unwrap();
    assert_eq!(
        unsafe { IsIconic(window) },
        0,
        "Restore must be applied before success"
    );
    eventually(|| unsafe { IsIconic(window) == 0 && IsWindowVisible(window) == 0 });
    assert!(is_transparent(window));
    drop(guard);
    eventually(|| !is_transparent(window));
    assert_eq!(unsafe { IsIconic(window) }, 0);
}

#[test]
fn blocked_restore_is_not_reported_ready_and_later_completes() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let guard = Guard::start(fixture.child.id()).unwrap();
    guard.minimize(true).unwrap();
    fixture.command("block");
    let started = Instant::now();
    assert!(guard.minimize(false).is_err());
    assert!(started.elapsed() < Duration::from_millis(2500));
    eventually(|| unsafe { IsIconic(window) == 0 && IsWindowVisible(window) == 0 });
    guard.minimize(false).unwrap();
    assert!(is_transparent(window));
}

#[test]
fn timed_out_minimize_cannot_overtake_a_later_restore() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let guard = Guard::start(fixture.child.id()).unwrap();
    fixture.command("block");
    assert!(guard.minimize(true).is_err());
    let started = Instant::now();
    let result = guard.minimize(false);
    assert!(result.is_err() || started.elapsed() >= Duration::from_secs(1));
    eventually(|| unsafe { IsIconic(window) == 0 && IsWindowVisible(window) == 0 });
    guard.minimize(false).unwrap();
    thread::sleep(Duration::from_millis(1100));
    assert_eq!(unsafe { IsIconic(window) }, 0);
}

#[test]
fn dropping_a_minimizing_guard_does_not_leave_a_previously_normal_window_minimized() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let guard = Guard::start(fixture.child.id()).unwrap();
    guard.minimize(true).unwrap();
    eventually(|| unsafe { IsIconic(window) != 0 });
    drop(guard);
    eventually(|| unsafe { IsIconic(window) == 0 } && !is_transparent(window));
}

#[test]
fn native_guard_survives_show_recreation_capture_and_restores_on_stop() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let first = fixture.window();
    assert!(!is_transparent(first));
    let guard = Guard::start(fixture.child.id()).unwrap();
    assert!(
        is_transparent(first),
        "Protection must be ready before hosting begins"
    );
    {
        let _capture = crate::window_capture::window_access();
        fixture.command("show");
        assert!(
            is_transparent(first),
            "ShowWindow must not reveal the game while capture is busy"
        );
    }
    fixture.command("reset");
    eventually(|| unsafe {
        GetWindowLongPtrW(first, GWL_EXSTYLE) & SHELL
            == (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT) as isize
    });
    let second = fixture.command("replace");
    eventually(|| is_transparent(second));
    fixture.command("show");
    assert!(is_transparent(second));
    drop(guard);
    eventually(|| !is_transparent(second) && unsafe { IsWindowVisible(second) } != 0);
    assert_ne!(unsafe { IsWindowEnabled(second) }, 0);
    assert_eq!(
        unsafe { GetWindowLongPtrW(second, GWL_EXSTYLE) } & SHELL,
        WS_EX_APPWINDOW as isize
    );
}

#[test]
fn successful_launcher_handoff_restores_input_but_keeps_the_window_hidden() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("show");
    let guard = Guard::start(fixture.child.id()).unwrap();
    guard.keep_hidden();
    drop(guard);
    eventually(|| unsafe { GetPropW(window, MARK.as_ptr()).is_null() });
    assert_eq!(unsafe { IsWindowVisible(window) }, 0);
    assert_ne!(unsafe { IsWindowEnabled(window) }, 0);
    assert!(!is_transparent(window));
    fixture.command("show");
    assert_ne!(unsafe { IsWindowVisible(window) }, 0);
}

#[test]
fn launcher_remains_protected_when_it_shows_or_recreates_windows_after_connection() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let first = fixture.window();
    fixture.command("launcher");
    let guard = Guard::start(fixture.child.id()).unwrap();
    let mut launch = crate::background_launcher::Launch::with_test_guard(fixture.child.id(), guard);
    launch.complete();
    fixture.command("show");
    eventually(|| unsafe { IsWindowVisible(first) } == 0);
    let second = fixture.command("replace");
    fixture.command("launcher");
    fixture.command("show");
    eventually(|| unsafe { IsWindowVisible(second) } == 0);
    drop(launch);
    eventually(|| unsafe { GetPropW(second, MARK.as_ptr()).is_null() });
    assert_eq!(unsafe { IsWindowVisible(second) }, 0);
    assert_ne!(unsafe { IsWindowEnabled(second) }, 0);
}

#[test]
fn a_dead_process_releases_its_persistent_window_watcher() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let guard = Guard::start(fixture.child.id()).unwrap();
    fixture.child.kill().unwrap();
    fixture.child.wait().unwrap();
    eventually(|| guard.check().is_err());
}

#[test]
fn cancelled_launch_restores_a_window_created_hidden_then_shown() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("launcher");
    assert_eq!(unsafe { IsWindowVisible(window) }, 0);
    let guard = Guard::start(fixture.child.id()).unwrap();
    fixture.command("show");
    eventually(|| unsafe { IsWindowVisible(window) } == 0);
    drop(guard);
    eventually(|| !is_transparent(window) && unsafe { IsWindowVisible(window) } != 0);
}

#[test]
fn stop_never_waits_for_capture_and_cleanup_retains_exclusive_ownership() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let guard = Guard::start(fixture.child.id()).unwrap();
    let capture = crate::window_capture::window_access();
    let started = Instant::now();
    drop(guard);
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(Guard::start(fixture.child.id()).is_err());
    drop(capture);
    eventually(|| !is_transparent(window));
}

#[test]
fn protection_start_has_a_deadline_when_the_target_does_not_pump_messages() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("block");
    let started = Instant::now();
    let guard = Guard::start(fixture.child.id());
    assert!(started.elapsed() < Duration::from_millis(2500));
    assert!(
        guard.is_err(),
        "A blocked window must not be reported ready"
    );
    drop(guard);
    thread::sleep(Duration::from_secs(2));
    eventually(|| !is_transparent(window));
}

#[test]
fn cold_start_waits_for_window_initialization_within_the_launch_deadline() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("block");
    let started = Instant::now();
    let guard = Guard::start_until(
        fixture.child.id(),
        &AtomicBool::new(false),
        started + Duration::from_secs(8),
    )
    .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(is_transparent(window));
    assert_eq!(unsafe { IsWindowEnabled(window) }, 0);
    drop(guard);
    eventually(|| !is_transparent(window) && unsafe { IsWindowEnabled(window) } != 0);
}

#[test]
fn cancelling_cold_start_returns_promptly_and_restores_after_the_window_recovers() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("block");
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = stop.clone();
    let setter = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        cancel.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    assert!(
        Guard::start_until(fixture.child.id(), &stop, started + Duration::from_secs(60)).is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    setter.join().unwrap();
    assert!(
        Guard::start(fixture.child.id()).is_err(),
        "Cleanup must retain the lease"
    );
    thread::sleep(Duration::from_secs(4));
    eventually(|| unsafe { GetPropW(window, MARK.as_ptr()).is_null() });
    assert!(!is_transparent(window));
    assert_ne!(unsafe { IsWindowEnabled(window) }, 0);
}

#[test]
fn existing_per_pixel_alpha_is_not_replaced_by_constant_opacity() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    fixture.command("perpixel");
    assert!(layered_attributes(window).is_none());
    let guard = Guard::start(fixture.child.id()).unwrap();
    assert!(layered_attributes(window).is_none());
    assert!(
        !protected(window),
        "Preview must not replace per-pixel alpha either"
    );
    {
        let _capture = crate::window_capture::window_access();
        fixture.command("show");
        eventually(|| unsafe { IsWindowVisible(window) } == 0);
    }
    drop(guard);
    eventually(|| unsafe { GetPropW(window, MARK.as_ptr()).is_null() });
    assert!(layered_attributes(window).is_none());
    assert_ne!(
        unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } & WS_EX_LAYERED as isize,
        0
    );
}

#[test]
fn startup_waits_for_temporary_preview_presentation_to_be_restored() {
    let _test = TEST_PRESENTATION.lock().unwrap();
    let mut fixture = Fixture::start();
    let window = fixture.window();
    let capture = crate::window_capture::window_access();
    fixture.command("preview");
    let pid = fixture.child.id();
    let worker = thread::spawn(move || Guard::start(pid));
    thread::sleep(Duration::from_millis(100));
    assert!(
        !worker.is_finished(),
        "Startup must not snapshot preview's zero opacity"
    );
    fixture.command("endpreview");
    drop(capture);
    let guard = worker.join().unwrap().unwrap();
    drop(guard);
    eventually(|| !is_transparent(window) && unsafe { IsWindowVisible(window) } != 0);
    assert_eq!(
        unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } & SHELL,
        WS_EX_APPWINDOW as isize
    );
}

use super::*;
use std::{
    io::{BufRead, Write},
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    time::Instant,
};

unsafe extern "C" {
    fn ghost_audio_fixture_open(muted: i32) -> *mut c_void;
    fn ghost_audio_fixture_muted(fixture: *mut c_void) -> i32;
    fn ghost_audio_fixture_close(fixture: *mut c_void);
}

struct Session(*mut c_void);
impl Session {
    fn new(muted: bool) -> Self {
        let raw = unsafe { ghost_audio_fixture_open(i32::from(muted)) };
        assert!(
            !raw.is_null(),
            "Native audio proof requires an active audio endpoint"
        );
        Self(raw)
    }
    fn muted(&self) -> bool {
        unsafe { ghost_audio_fixture_muted(self.0) == 1 }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            ghost_audio_fixture_close(self.0);
        }
    }
}

#[test]
fn native_audio_fixture() {
    if std::env::var_os("GHOST_AUDIO_FIXTURE").is_none() {
        return;
    }
    let first = Session::new(false);
    let muted = Session::new(true);
    let mut later = None;
    println!("\nAUDIO ready");
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        match line.unwrap().as_str() {
            "new" => {
                later = Some(Session::new(false));
                println!("AUDIO created");
            }
            "state" => println!(
                "AUDIO {} {} {}",
                first.muted(),
                muted.muted(),
                later.as_ref().is_some_and(Session::muted)
            ),
            "quit" => break,
            _ => {}
        }
        std::io::stdout().flush().unwrap();
    }
}

#[test]
fn native_mute_covers_new_sessions_and_restores_without_muting_other_processes() {
    let _serial = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let unaffected = Session::new(false);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background_audio::tests::native_audio_fixture",
            "--nocapture",
        ])
        .env("GHOST_AUDIO_FIXTURE", "1")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let output = child.stdout.take().unwrap();
    thread::spawn(move || {
        for line in std::io::BufReader::new(output)
            .lines()
            .map_while(Result::ok)
        {
            if line.starts_with("AUDIO ") {
                let _ = tx.send(line);
            }
        }
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        "AUDIO ready"
    );
    let mut silence = Silence::default();
    silence.target(Some(child.id()));
    let guard = Guard::start(child.id()).unwrap();
    // Wait until the launcher owner has joined the hosting owner's lease.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if sessions()
            .lock()
            .unwrap()
            .get(&child.id())
            .is_some_and(|entry| entry.users == 2)
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    writeln!(input, "state").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "AUDIO true true false"
    );
    writeln!(input, "new").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "AUDIO created"
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        writeln!(input, "state").unwrap();
        let state = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        if state == "AUDIO true true true" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "New audio session was not muted: {state}"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(!unaffected.muted());
    drop(guard);
    writeln!(input, "state").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "AUDIO true true true"
    );
    // Restarting and stopping hosting must not replace the original mute state.
    drop(Guard::start(child.id()).unwrap());
    drop(silence);
    writeln!(input, "state").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "AUDIO false true false"
    );
    assert!(!unaffected.muted());
    writeln!(input, "quit").unwrap();
    assert!(child.wait().unwrap().success());
}

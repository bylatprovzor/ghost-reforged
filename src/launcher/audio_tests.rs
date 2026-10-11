use super::{
    app::{App, Message},
    process::{GameState, Observation},
    tray,
};
use std::{
    ffi::c_void,
    thread,
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn ghost_audio_fixture_open(muted: i32) -> *mut c_void;
    fn ghost_audio_fixture_muted(fixture: *mut c_void) -> i32;
    fn ghost_audio_fixture_close(fixture: *mut c_void);
}

struct Session(*mut c_void);
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            ghost_audio_fixture_close(self.0);
        }
    }
}

fn wait_for(session: &Session, muted: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { ghost_audio_fixture_muted(session.0) } != muted {
        assert!(
            Instant::now() < deadline,
            "Audio did not reach the requested mute state"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn idle_launcher_mutes_until_quit_including_close_to_tray_and_stop_hosting() {
    let raw = unsafe { ghost_audio_fixture_open(0) };
    assert!(
        !raw.is_null(),
        "Native audio proof requires an active audio endpoint"
    );
    let session = Session(raw);
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    assert!(app.worker.is_none());
    let _ = app.update(Message::ProcessObserved(Observation {
        state: GameState::Visible,
        pid: Some(std::process::id()),
    }));
    wait_for(&session, 1);
    app.tray_available = true;
    let _ = app.update(Message::CloseRequested(iced::window::Id::unique()));
    assert!(app.hidden);
    let _ = app.update(Message::Stop);
    wait_for(&session, 1);
    let _ = app.update(Message::Tray(tray::Event::Open));
    wait_for(&session, 1);
    drop(app);
    wait_for(&session, 0);
}

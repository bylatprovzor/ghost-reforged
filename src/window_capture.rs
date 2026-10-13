//! Window-only thumbnails. No desktop capture or game input.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

static SUPPRESSED_PID: AtomicU32 = AtomicU32::new(0);

pub fn render_paused(pid: u32) -> bool {
    pid != 0 && SUPPRESSED_PID.load(Ordering::SeqCst) == pid
}

#[cfg(windows)]
pub(crate) struct Suppression(u32);
#[cfg(windows)]
impl Suppression {
    pub fn new(pid: u32) -> Self {
        SUPPRESSED_PID.store(pid, Ordering::SeqCst);
        Self(pid)
    }
}
#[cfg(windows)]
impl Drop for Suppression {
    fn drop(&mut self) {
        let _ = SUPPRESSED_PID.compare_exchange(self.0, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

static WINDOW_ACCESS: Mutex<()> = Mutex::new(());

pub(crate) fn window_access() -> MutexGuard<'static, ()> {
    WINDOW_ACCESS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[cfg(windows)]
pub(crate) fn try_window_access() -> Option<MutexGuard<'static, ()>> {
    match WINDOW_ACCESS.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(windows)]
#[path = "window_capture_windows.rs"]
mod native;
pub fn capture(pid: u32) -> Result<Frame, String> {
    capture_preview(pid, false)
}

pub fn capture_preview(pid: u32, expanded: bool) -> Result<Frame, String> {
    if render_paused(pid) {
        return Err("Preview paused during low-resource hosting.".into());
    }
    capture_sized(pid, if expanded { 1280 } else { 640 })
}

#[cfg(windows)]
fn capture_sized(pid: u32, width: i32) -> Result<Frame, String> {
    native::capture(pid, width)
}

#[cfg(not(windows))]
fn capture_sized(_: u32, _: i32) -> Result<Frame, String> {
    Err("Window previews require Windows.".into())
}

#[cfg(all(test, windows))]
#[test]
fn low_resource_capture_is_rejected_before_opening_or_showing_any_window() {
    let _serial = crate::background_window::TEST_PRESENTATION.lock().unwrap();
    let pause = Suppression::new(123);
    assert!(render_paused(123));
    assert!(!render_paused(456));
    for expanded in [false, true] {
        assert_eq!(
            capture_preview(123, expanded).unwrap_err(),
            "Preview paused during low-resource hosting."
        );
    }
    drop(pause);
    assert!(!render_paused(123));
}

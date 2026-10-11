//! Window-only thumbnails. No desktop capture or game input.
use std::sync::{Mutex, MutexGuard};

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

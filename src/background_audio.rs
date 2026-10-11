//! Session volume belongs to the managed game process, never to the output device.
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::Duration,
};

unsafe extern "C" {
    fn ghost_audio_open(pid: u32) -> *mut c_void;
    fn ghost_audio_refresh(context: *mut c_void);
    fn ghost_audio_close(context: *mut c_void);
}

struct Worker {
    stop: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Worker {
    fn start(pid: u32) -> Result<Self, String> {
        let (stop, requests) = mpsc::channel();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("ghost-audio".into())
            .spawn(move || {
                let context = unsafe { ghost_audio_open(pid) };
                let _ = ready.send(!context.is_null());
                if context.is_null() {
                    return;
                }
                while matches!(
                    requests.recv_timeout(Duration::from_secs(2)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    unsafe {
                        ghost_audio_refresh(context);
                    }
                }
                unsafe {
                    ghost_audio_close(context);
                }
            })
            .map_err(|_| "Cannot start Warcraft III audio control.")?;
        let guard = Self {
            stop: Some(stop),
            worker: Some(worker),
        };
        if result.recv_timeout(Duration::from_secs(3)) == Ok(true) {
            Ok(guard)
        } else {
            Err("Cannot mute Warcraft III audio sessions.".into())
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Entry {
    users: usize,
    _worker: Worker,
}

fn sessions() -> &'static Mutex<HashMap<u32, Entry>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u32, Entry>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}

/// A process has one native mute owner even when the launcher and host overlap.
pub(crate) struct Guard(u32);

impl Guard {
    pub(crate) fn start(pid: u32) -> Result<Self, String> {
        let mut sessions = sessions().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = sessions.get_mut(&pid) {
            entry.users += 1;
        } else {
            sessions.insert(
                pid,
                Entry {
                    users: 1,
                    _worker: Worker::start(pid)?,
                },
            );
        }
        Ok(Self(pid))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut sessions = sessions().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = sessions.get_mut(&self.0) {
            entry.users -= 1;
            if entry.users == 0 {
                // Restore before allowing another owner to read the previous mute state.
                drop(sessions.remove(&self.0));
            }
        }
    }
}

/// Keeps the selected game's audio muted for the entire launcher lifetime.
/// COM startup and restoration stay off the GUI thread.
pub struct Silence {
    requests: Option<mpsc::Sender<Option<u32>>>,
    errors: mpsc::Receiver<String>,
    worker: Option<thread::JoinHandle<()>>,
    target: Option<u32>,
}

impl Default for Silence {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let (errors, reports) = mpsc::channel();
        let failures = errors.clone();
        let worker = thread::Builder::new()
            .name("ghost-silence".into())
            .spawn(move || {
                let mut target = None;
                let mut guard: Option<Guard> = None;
                let mut reported = false;
                loop {
                    let request = if target.is_some() && guard.is_none() {
                        rx.recv_timeout(Duration::from_secs(2))
                    } else {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    match request {
                        Ok(next) if next != target => {
                            guard = None;
                            target = next;
                            reported = false;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        _ => {}
                    }
                    if guard.is_none()
                        && let Some(pid) = target
                    {
                        match Guard::start(pid) {
                            Ok(muted) => guard = Some(muted),
                            Err(error) if !reported => {
                                let _ = errors.send(error);
                                reported = true;
                            }
                            Err(_) => {}
                        }
                    }
                }
            })
            .ok();
        if worker.is_none() {
            let _ = failures.send("Cannot start Warcraft III audio control.".into());
        }
        Self {
            requests: Some(tx),
            errors: reports,
            worker,
            target: None,
        }
    }
}

impl Silence {
    pub fn target(&mut self, pid: Option<u32>) {
        if pid != self.target {
            self.target = pid;
            if let Some(requests) = &self.requests {
                let _ = requests.send(pid);
            }
        }
    }

    pub fn error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "background_audio_tests.rs"]
mod tests;

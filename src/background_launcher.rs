//! Suppress only the verified Battle.net executable during a bot-requested launch.
mod readiness;
use crate::background_render::process::{Process, same_path};
use crate::background_window::Guard;
use std::{
    collections::HashMap,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, RemoteDesktop::ProcessIdToSessionId},
};

#[derive(Default)]
pub(crate) struct Launch {
    path: Option<PathBuf>,
    guards: HashMap<u32, Guard>,
    request: Request,
    polled: Option<Instant>,
    startup: readiness::Generation,
}

impl Launch {
    pub(crate) fn prepare_start(
        &mut self,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        let Some(root) = std::env::var_os("ProgramFiles(x86)") else {
            return Ok(());
        };
        let launcher = PathBuf::from(root).join("Battle.net/Battle.net.exe");
        if !launcher.is_file() {
            return Ok(());
        }
        self.prepare(&launcher, stop, deadline)?;
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut bare_start_requested = false;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            self.poll(stop, deadline)?;
            let launchers = processes(&launcher)?;
            // A short-lived --exec process may coexist with the main launcher.
            // The oldest matching process in our session owns initialization.
            if let Some(process) = launchers.iter().min_by_key(|process| process.created) {
                bare_start_requested = true;
                if self.startup.ready(process.pid, process.created) && process.running() {
                    return Ok(());
                }
            } else {
                // Discard readiness from a launcher that has exited. A controller
                // restart also reconstructs this evidence from the native lifetime.
                self.startup = readiness::Generation::default();
                if !bare_start_requested {
                    // Cold --exec may be consumed before GameController exists.
                    crate::background::spawn_hidden(&launcher, "")?;
                    bare_start_requested = true;
                }
            }
            if Instant::now() >= deadline {
                // No game command has been sent. A launcher that is absent can
                // safely receive a fresh bare startup on the next attempt.
                return Err("Battle.net has not completed startup yet.".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    #[cfg(test)]
    pub(crate) fn with_test_guard(pid: u32, guard: Guard) -> Self {
        Self {
            guards: HashMap::from([(pid, guard)]),
            ..Default::default()
        }
    }
    pub(crate) fn prepare(
        &mut self,
        path: &Path,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        self.path = Some(path.to_owned());
        self.poll(stop, deadline)
    }

    pub(crate) fn poll(&mut self, stop: &AtomicBool, deadline: Instant) -> Result<(), String> {
        if self
            .polled
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        self.polled = Some(Instant::now());
        let Some(expected) = &self.path else {
            return Ok(());
        };
        self.guards.retain(|_, guard| guard.check().is_ok());
        let processes = processes(expected)?;
        self.request.observe_launcher(!processes.is_empty());
        for process in processes {
            let pid = process.pid;
            if let std::collections::hash_map::Entry::Vacant(entry) = self.guards.entry(pid) {
                entry.insert(
                    Guard::start_until(pid, stop, deadline)
                        .map_err(|error| format!("Battle.net: {error}"))?,
                );
            }
        }
        Ok(())
    }

    pub(crate) fn watch(&mut self, stop: &AtomicBool, deadline: Instant) -> Result<(), String> {
        if self.path.is_none()
            && let Some(root) = std::env::var_os("ProgramFiles(x86)")
        {
            self.path = Some(PathBuf::from(root).join("Battle.net/Battle.net.exe"));
        }
        self.poll(stop, deadline)
    }

    pub(crate) fn requested(&mut self) {
        self.request = Request {
            pending: true,
            launcher_seen: false,
        };
    }

    pub(crate) fn pending(&self) -> bool {
        self.request.pending
    }

    pub(crate) fn client_observed(&mut self) {
        self.request = Request::default();
        self.startup = readiness::Generation::default();
    }

    pub(crate) fn complete(&mut self) {
        // Restore styles and input, but leave the launcher in its notification area.
        // Failed/cancelled launches instead restore visibility for manual sign-in.
        for guard in self.guards.values() {
            guard.keep_hidden();
        }
        // Keep hooks alive for delayed launcher notifications and recreated windows.
    }
}

#[derive(Default)]
struct Request {
    pending: bool,
    launcher_seen: bool,
}

impl Request {
    fn observe_launcher(&mut self, present: bool) {
        // Battle.net may queue --exec during startup. Time alone cannot prove
        // that a request was lost: repeating it can create multiple clients.
        // A launcher that was seen and has exited cannot retain that queue.
        if self.pending && self.launcher_seen && !present {
            *self = Self::default();
        } else {
            self.launcher_seen |= present;
        }
    }
}

fn processes(expected: &Path) -> Result<Vec<Process>, String> {
    unsafe {
        let mut own_session = 0;
        if ProcessIdToSessionId(std::process::id(), &mut own_session) == 0 || own_session == 0 {
            return Err("The game launcher requires an interactive Windows session.".into());
        }
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("Cannot inspect the game launcher.".into());
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut result = Vec::new();
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..length])
                .eq_ignore_ascii_case("Battle.net.exe")
            {
                let mut session = 0;
                if ProcessIdToSessionId(entry.th32ProcessID, &mut session) != 0
                    && session == own_session
                    && let Ok(process) = Process::open(entry.th32ProcessID, false)
                    && same_path(&process.path, expected)
                    && process.running()
                {
                    result.push(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_launcher_never_receives_duplicate_launch_commands() {
        let mut launch = Launch::default();
        assert!(!launch.pending());
        launch.requested();
        // Polls and connection timeout/retry cycles retain the same request.
        for _ in 0..1800 {
            launch.request.observe_launcher(true);
            assert!(launch.pending());
        }
        launch.client_observed();
        assert!(!launch.pending());
        launch.requested();
        assert!(launch.pending());
    }

    #[test]
    fn only_a_confirmed_launcher_exit_releases_an_unanswered_request() {
        let mut launch = Launch::default();
        launch.requested();
        launch.request.observe_launcher(false);
        assert!(launch.pending());
        launch.request.observe_launcher(true);
        assert!(launch.pending());
        launch.request.observe_launcher(false);
        assert!(!launch.pending());
    }
}

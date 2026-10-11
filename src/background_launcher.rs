//! Suppress only the verified Battle.net executable during a bot-requested launch.
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
    System::{Diagnostics::ToolHelp::*, Threading::*},
};

#[derive(Default)]
pub(crate) struct Launch {
    path: Option<PathBuf>,
    guards: HashMap<u32, Guard>,
    retry: Retry,
}

impl Launch {
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
        let Some(expected) = &self.path else {
            return Ok(());
        };
        for pid in processes(expected)? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.guards.entry(pid) {
                entry.insert(
                    Guard::start_until(pid, stop, deadline)
                        .map_err(|error| format!("Battle.net: {error}"))?,
                );
            }
        }
        Ok(())
    }

    pub(crate) fn requested(&mut self) {
        self.retry.requested(Instant::now());
    }

    pub(crate) fn retry_if_needed(
        &mut self,
        game_absent: bool,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), String> {
        let now = Instant::now();
        if !stop.load(Ordering::SeqCst)
            && now < deadline
            && self.retry.due(game_absent, now)
            && let Some(path) = &self.path
        {
            // A cold Battle.net can consume --exec before its game catalog is
            // ready. Repeat only until a Warcraft process has been observed.
            crate::background::spawn(path, "--exec=\"launch W3\"")?;
            self.requested();
        }
        Ok(())
    }

    pub(crate) fn complete(&mut self) {
        // Restore styles and input, but leave the launcher in its notification area.
        // Failed/cancelled launches instead restore visibility for manual sign-in.
        for guard in self.guards.values() {
            guard.keep_hidden();
        }
        self.guards.clear();
        self.path = None;
    }
}

#[derive(Default)]
struct Retry {
    next: Option<Instant>,
    game_seen: bool,
}

impl Retry {
    fn requested(&mut self, now: Instant) {
        self.next = Some(now + Duration::from_secs(10));
    }

    fn due(&mut self, game_absent: bool, now: Instant) -> bool {
        self.game_seen |= !game_absent;
        !self.game_seen && self.next.is_some_and(|next| now >= next)
    }
}

fn processes(expected: &Path) -> Result<Vec<u32>, String> {
    unsafe {
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
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !process.is_null() {
                    let mut path = [0u16; 32768];
                    let mut size = path.len() as u32;
                    if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) != 0
                        && Path::new(&String::from_utf16_lossy(&path[..size as usize])) == expected
                    {
                        result.push(entry.th32ProcessID);
                    }
                    CloseHandle(process);
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
    fn lost_cold_launch_is_retried_until_a_process_exists_even_without_an_endpoint() {
        let now = Instant::now();
        let mut retry = Retry::default();
        assert!(!retry.due(true, now)); // Never launch on an attach-only path.
        retry.requested(now);
        assert!(!retry.due(true, now + Duration::from_secs(9)));
        assert!(retry.due(true, now + Duration::from_secs(10)));
        retry.requested(now + Duration::from_secs(10));
        assert!(!retry.due(true, now + Duration::from_secs(11)));
        assert!(!retry.due(false, now + Duration::from_secs(12)));
        assert!(!retry.due(true, now + Duration::from_secs(30))); // Do not relaunch a crashed client.
    }
}

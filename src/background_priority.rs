//! Keep a headless client's normal-priority work behind desktop/SSH control.
//! Retained handles protect restoration from PID reuse. Idle priorities are never raised.
use crate::background_render::process::{Process, processes, same_path};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS};

#[cfg(test)]
#[path = "background_priority_tests.rs"]
mod lifecycle_tests;

struct Held {
    process: Process,
    changed: bool,
}

impl Held {
    fn new(process: Process) -> Result<Self, String> {
        let changed = process.priority() == NORMAL_PRIORITY_CLASS;
        if changed {
            process.set_priority(BELOW_NORMAL_PRIORITY_CLASS)?;
        }
        Ok(Self { process, changed })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.changed
            && self.process.running()
            && self.process.priority() == BELOW_NORMAL_PRIORITY_CLASS
        {
            let _ = self.process.set_priority(NORMAL_PRIORITY_CLASS);
        }
    }
}

pub(crate) struct Guard {
    held: Vec<Held>,
    browser: PathBuf,
    polled: Option<Instant>,
}

impl Guard {
    pub fn new(pid: u32) -> Result<Self, String> {
        let process = Process::open_priority(pid)?;
        if !process
            .path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Warcraft III.exe"))
        {
            return Err("Background priority requires the Warcraft III client.".into());
        }
        let browser = process
            .path
            .parent()
            .ok_or("Missing client directory")?
            .join("BlizzardBrowser/BlizzardBrowser.exe");
        Ok(Self {
            held: vec![Held::new(process)?],
            browser,
            polled: None,
        })
    }

    pub fn poll(&mut self) -> Result<(), String> {
        if self
            .polled
            .is_some_and(|time| time.elapsed() < Duration::from_secs(5))
        {
            return Ok(());
        }
        self.polled = Some(Instant::now());
        if !self.held[0].process.running() {
            return Ok(());
        }
        let candidates = processes()?;
        let mut index = 0;
        while index < self.held.len() {
            let parent = &self.held[index].process;
            if !parent.running() {
                index += 1;
                continue;
            }
            let children: Vec<_> = candidates
                .iter()
                .filter(|(pid, parent_pid, name)| {
                    *parent_pid == parent.pid
                        && name.eq_ignore_ascii_case("BlizzardBrowser.exe")
                        && !self
                            .held
                            .iter()
                            .any(|held| held.process.pid == *pid && held.process.running())
                })
                .map(|(pid, _, _)| *pid)
                .collect();
            for pid in children {
                // A browser child can disappear between enumeration and opening.
                let Ok(child) = Process::open_priority(pid) else {
                    continue;
                };
                if same_path(&child.path, &self.browser)
                    && child.child_of(&self.held[index].process)?
                {
                    self.held.push(Held::new(child)?);
                }
            }
            index += 1;
        }
        // Retain the game at index zero; discard dead browser generations.
        let mut index = 1;
        while index < self.held.len() {
            if self.held[index].process.running() {
                index += 1;
            } else {
                self.held.remove(index);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, IDLE_PRIORITY_CLASS};

    #[test]
    fn priority_fixture() {
        if std::env::var_os("GHOST_PRIORITY_FIXTURE").is_some() {
            let _ = std::io::stdin().read(&mut [0u8]);
        }
    }

    #[test]
    fn native_priority_is_reversible_never_raises_idle_and_preserves_external_changes() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "background_priority::tests::priority_fixture",
                "--nocapture",
            ])
            .env("GHOST_PRIORITY_FIXTURE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        let observer = Process::open_priority(child.id()).unwrap();
        observer.set_priority(NORMAL_PRIORITY_CLASS).unwrap();
        let changed = Held::new(Process::open_priority(child.id()).unwrap()).unwrap();
        assert_eq!(observer.priority(), BELOW_NORMAL_PRIORITY_CLASS);
        drop(changed);
        assert_eq!(observer.priority(), NORMAL_PRIORITY_CLASS);
        observer.set_priority(IDLE_PRIORITY_CLASS).unwrap();
        let idle = Held::new(Process::open_priority(child.id()).unwrap()).unwrap();
        assert_eq!(observer.priority(), IDLE_PRIORITY_CLASS);
        drop(idle);
        assert_eq!(observer.priority(), IDLE_PRIORITY_CLASS);
        observer.set_priority(NORMAL_PRIORITY_CLASS).unwrap();
        let changed = Held::new(Process::open_priority(child.id()).unwrap()).unwrap();
        observer.set_priority(IDLE_PRIORITY_CLASS).unwrap();
        drop(changed);
        assert_eq!(observer.priority(), IDLE_PRIORITY_CLASS);
        assert!(Guard::new(child.id()).is_err());
        child.stdin.take();
        assert!(child.wait().unwrap().success());
    }
}

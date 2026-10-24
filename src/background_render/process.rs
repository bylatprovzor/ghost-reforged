//! Handle-based ownership: never resume a reused PID or another client's browser.
use std::{
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT},
    System::{Diagnostics::ToolHelp::*, Threading::*},
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

pub(crate) struct Process {
    handle: HANDLE,
    pub pid: u32,
    pub created: u64,
    pub path: PathBuf,
    suspended: bool,
}

impl Process {
    pub fn open(pid: u32, suspend: bool) -> Result<Self, String> {
        Self::open_access(pid, if suspend { PROCESS_SUSPEND_RESUME } else { 0 })
    }

    pub(crate) fn open_priority(pid: u32) -> Result<Self, String> {
        Self::open_access(pid, PROCESS_SET_INFORMATION)
    }

    pub(crate) fn priority(&self) -> u32 {
        unsafe { GetPriorityClass(self.handle) }
    }

    pub(crate) fn set_priority(&self, priority: u32) -> Result<(), String> {
        if unsafe { SetPriorityClass(self.handle, priority) } == 0 {
            return Err("Cannot set background process priority.".into());
        }
        Ok(())
    }

    fn open_access(pid: u32, extra: u32) -> Result<Self, String> {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | extra;
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err("Cannot open the render process.".into());
        }
        let mut result = Self {
            handle,
            pid,
            created: 0,
            path: PathBuf::new(),
            suspended: false,
        };
        let mut times: [FILETIME; 4] = unsafe { zeroed() };
        if unsafe {
            GetProcessTimes(
                handle,
                &mut times[0],
                &mut times[1],
                &mut times[2],
                &mut times[3],
            )
        } == 0
        {
            return Err("Cannot identify the render process lifetime.".into());
        }
        result.created =
            (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime);
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
            return Err("Cannot identify the render executable.".into());
        }
        result.path = String::from_utf16_lossy(&path[..length as usize]).into();
        Ok(result)
    }

    pub fn running(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT }
    }

    pub fn child_of(&self, parent: &Self) -> Result<bool, String> {
        // Discovery is only a hint: open handles first, then take a fresh ancestry
        // snapshot. Both retained lifetimes must still be alive after that snapshot;
        // a recycled PID can therefore never borrow the old process's parent.
        let actual_parent = parent_pid(self.pid)?;
        Ok(actual_parent == parent.pid
            && self.created >= parent.created
            && self.running()
            && parent.running())
    }

    fn suspend(&mut self) -> Result<(), String> {
        if !self.suspended && self.running() {
            if unsafe { NtSuspendProcess(self.handle) } < 0 {
                return Err("Cannot pause the embedded browser.".into());
            }
            self.suspended = true;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<(), String> {
        if self.suspended {
            if self.running() && unsafe { NtResumeProcess(self.handle) } < 0 {
                return Err("Cannot resume the embedded browser.".into());
            }
            self.suspended = false;
        }
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.resume();
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let (mut a, mut b) = (a.components(), b.components());
    loop {
        match (a.next(), b.next()) {
            (Some(a), Some(b)) if a.as_os_str().eq_ignore_ascii_case(b.as_os_str()) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

struct Snapshot(HANDLE);
impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) fn processes() -> Result<Vec<(u32, u32, String)>, String> {
    let snapshot = Snapshot(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snapshot.0 == INVALID_HANDLE_VALUE {
        return Err("Cannot enumerate the embedded browser.".into());
    }
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    let mut result = Vec::new();
    while more != 0 {
        let length = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        result.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..length]),
        ));
        more = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    Ok(result)
}

pub(super) fn parent_pid(pid: u32) -> Result<u32, String> {
    processes()?
        .into_iter()
        .find(|p| p.0 == pid)
        .map(|p| p.1)
        .ok_or_else(|| "The render guardian parent exited.".into())
}

pub(super) struct Family {
    game: Process,
    browser_path: PathBuf,
    paused: Vec<Process>,
}

impl Family {
    pub fn new(pid: u32, created: u64) -> Result<Self, String> {
        let game = Process::open(pid, false)?;
        if game.created != created
            || !game
                .path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("Warcraft III.exe"))
        {
            return Err("The render target is not the retained Warcraft III client.".into());
        }
        let browser_path = game
            .path
            .parent()
            .ok_or("Missing client directory")?
            .join("BlizzardBrowser/BlizzardBrowser.exe");
        Ok(Self {
            game,
            browser_path,
            paused: Vec::new(),
        })
    }

    pub fn running(&self) -> bool {
        self.game.running()
    }

    pub fn pause(&mut self) -> Result<(), String> {
        let result = self.pause_inner();
        if result.is_err() {
            self.resume()?;
        }
        result
    }

    fn pause_inner(&mut self) -> Result<(), String> {
        if !self.running() {
            return Err("Warcraft III exited.".into());
        }
        self.paused.retain(Process::running);
        let mut candidates = processes()?;
        let mut parents = vec![(self.game.pid, self.game.created)];
        // Parent-first traversal stops the browser root before its GPU/renderer children.
        while let Some((parent, created)) = parents.pop() {
            let alive = if parent == self.game.pid {
                self.game.running()
            } else {
                self.paused
                    .iter()
                    .any(|p| p.pid == parent && p.created == created && p.running())
            };
            if !alive {
                continue;
            }
            let children: Vec<_> = candidates
                .iter()
                .filter(|p| p.1 == parent && p.2.eq_ignore_ascii_case("BlizzardBrowser.exe"))
                .map(|p| p.0)
                .collect();
            candidates.retain(|p| !children.contains(&p.0));
            for pid in children {
                if let Some(held) = self.paused.iter().find(|p| p.pid == pid && p.running()) {
                    parents.push((held.pid, held.created));
                    continue;
                }
                let mut process = Process::open(pid, true)?;
                let retained_parent = if parent == self.game.pid {
                    &self.game
                } else {
                    self.paused
                        .iter()
                        .find(|p| p.pid == parent && p.created == created)
                        .ok_or("The retained browser parent exited")?
                };
                if !same_path(&process.path, &self.browser_path)
                    || !process.child_of(retained_parent)?
                {
                    continue;
                }
                process.suspend()?;
                parents.push((process.pid, process.created));
                self.paused.push(process);
            }
        }
        if self.paused.is_empty() {
            return Err("Embedded browser is not ready for low-resource hosting.".into());
        }
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), String> {
        let mut error = None;
        for process in self.paused.iter_mut().rev() {
            if let Err(failed) = process.resume() {
                error = Some(failed);
            }
        }
        self.paused.retain(|p| p.suspended);
        error.map_or(Ok(()), Err)
    }
}

impl Drop for Family {
    fn drop(&mut self) {
        let _ = self.resume();
    }
}

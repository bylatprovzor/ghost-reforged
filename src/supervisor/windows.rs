use super::{
    policy::{self, Backoff, Decision},
    state::{State, WorkerIdentity},
};
use crate::console;
use std::{
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, WAIT_TIMEOUT},
    System::Threading::{
        CREATE_NO_WINDOW, GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    },
};

enum Process {
    Spawned(Child),
    Adopted { handle: OwnedHandle, pid: u32 },
}
impl Process {
    fn pid(&self) -> u32 {
        match self {
            Self::Spawned(child) => child.id(),
            Self::Adopted { pid, .. } => *pid,
        }
    }
    fn handle(&self) -> HANDLE {
        match self {
            Self::Spawned(child) => child.as_raw_handle(),
            Self::Adopted { handle, .. } => handle.as_raw_handle(),
        }
    }
    fn exit_code(&self) -> Result<Option<u32>, String> {
        let wait = unsafe { WaitForSingleObject(self.handle(), 0) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        let mut code = 0;
        if wait != 0 || unsafe { GetExitCodeProcess(self.handle(), &mut code) } == 0 {
            return Err("Cannot inspect the retained controller process".into());
        }
        Ok(Some(code))
    }
    fn terminate(&self) -> Result<(), String> {
        if self.exit_code()?.is_some() {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.handle(), 1) } == 0 {
            return Err("Cannot stop the stalled controller".into());
        }
        if unsafe { WaitForSingleObject(self.handle(), 5000) } != 0 {
            return Err("Controller exit is not confirmed; replacement was not started".into());
        }
        Ok(())
    }
    fn identity(&self) -> Result<WorkerIdentity, String> {
        Ok(WorkerIdentity {
            version: 1,
            pid: self.pid(),
            born: birth(self.handle())?,
        })
    }
}

fn birth(handle: HANDLE) -> Result<u64, String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err("Cannot verify controller creation time".into());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn session(pid: u32) -> Result<u32, String> {
    let mut value = 0;
    if unsafe { ProcessIdToSessionId(pid, &mut value) } == 0 {
        return Err("Cannot identify the Windows session".into());
    }
    Ok(value)
}

fn timestamp() -> u64 {
    116_444_736_000_000_000
        + (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            / 100) as u64
}

fn adopt(
    executable: &Path,
    pid: u32,
    before: u64,
    expected_birth: Option<u64>,
) -> Result<Option<Process>, String> {
    if pid == 0 {
        return Err("An incompatible controller is already running".into());
    }
    if pid == std::process::id() {
        let current_birth = birth(unsafe { GetCurrentProcess() })?;
        if expected_birth.is_some_and(|expected| expected != current_birth) {
            return Ok(None);
        }
        return Err("An incompatible controller is already running".into());
    }
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            0,
            pid,
        )
    };
    if raw.is_null() {
        if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            return Ok(None);
        }
        return Err("Cannot retain the existing controller".into());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if unsafe { WaitForSingleObject(raw, 0) } == 0 {
        return Ok(None);
    }
    let born = birth(raw)?;
    if expected_birth.is_some_and(|expected| expected != born) {
        return Ok(None);
    }
    if born > before || session(pid)? != session(std::process::id())? {
        return Err("Existing controller belongs to another Windows session".into());
    }
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(raw, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err("Cannot verify the existing controller".into());
    }
    let actual = PathBuf::from(String::from_utf16_lossy(&path[..length as usize]));
    if actual.canonicalize().ok().as_deref() != Some(executable)
        || unsafe { WaitForSingleObject(raw, 0) } != WAIT_TIMEOUT
    {
        return Err("The existing controller identity does not match this supervisor".into());
    }
    Ok(Some(Process::Adopted { handle, pid }))
}

fn spawn(executable: &Path, directory: &Path) -> Result<Process, String> {
    Command::new(executable)
        .arg("--controller")
        .arg("--data-dir")
        .arg(directory)
        .current_dir(
            executable
                .parent()
                .ok_or("Executable directory is unavailable")?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(Process::Spawned)
        .map_err(|_| "Cannot start the controller".into())
}

pub(super) fn run(directory: &Path) -> Result<(), String> {
    let state = State::open(directory)?;
    if session(std::process::id())? == 0 {
        state.record("interactive_session_required", None, 0)?;
        return Err("Start the supervisor in a logged-on Windows user session".into());
    }
    let executable = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|_| "Cannot identify supervisor executable")?;
    let directory = directory
        .canonicalize()
        .map_err(|_| "Profile directory is unavailable")?;
    let mut backoff = Backoff::default();
    let mut restarts: u32 = 0;
    loop {
        let before = timestamp();
        let previous = match console::health(&directory) {
            Ok(health) => {
                if health.version != 1 {
                    return Err("Unsupported controller health version".into());
                }
                adopt(&executable, health.pid, before, None)?
            }
            Err(_) => match state.worker()? {
                Some(identity) => adopt(&executable, identity.pid, before, Some(identity.born))?,
                None => None,
            },
        };
        let controller = match previous {
            Some(controller) => {
                state.record("controller_adopted", Some(controller.pid()), restarts)?;
                controller
            }
            None => {
                state.record("controller_starting", None, restarts)?;
                match spawn(&executable, &directory) {
                    Ok(controller) => controller,
                    Err(_) => {
                        state.record("controller_start_failed", None, restarts)?;
                        thread::sleep(backoff.next());
                        continue;
                    }
                }
            }
        };
        // Retain identity before monitoring, so a supervisor restart can recover
        // even when the existing controller's IPC is frozen.
        state.retain_worker(&controller.identity()?)?;
        let mut last_reply = Instant::now();
        let mut ready = false;
        let mut quitting = None;
        let mut reported = "";
        loop {
            if let Some(code) = controller.exit_code()? {
                if code == 0 || quitting.is_some() || console::quit_requested(&directory)? {
                    state.record("stopped", None, restarts)?;
                    return Ok(());
                }
                state.record("controller_exited", None, restarts)?;
                break;
            }
            match console::health(&directory) {
                Ok(health) => {
                    backoff.observe(controller.pid(), Some(&health), Instant::now());
                    match policy::inspect(controller.pid(), &health) {
                        Decision::Foreign => {
                            state.record("controller_identity_mismatch", None, restarts)?;
                            return Err(
                                "Another controller owns the endpoint; no process was stopped"
                                    .into(),
                            );
                        }
                        Decision::Restart => {
                            state.record("worker_stalled", Some(controller.pid()), restarts)?;
                            controller.terminate()?;
                            break;
                        }
                        Decision::Stop => {
                            quitting.get_or_insert_with(Instant::now);
                        }
                        Decision::Wait => {}
                    }
                    last_reply = Instant::now();
                    ready = true;
                    let event = if quitting.is_some() {
                        "stopping"
                    } else if health.worker.is_some() {
                        "worker_responsive"
                    } else {
                        "idle"
                    };
                    if event != reported {
                        state.record(event, Some(controller.pid()), restarts)?;
                        reported = event;
                    }
                }
                Err(_) => {
                    backoff.observe(controller.pid(), None, Instant::now());
                    let limit = if ready {
                        Duration::from_secs(policy::STALL_LIMIT)
                    } else {
                        policy::START_GRACE
                    };
                    if last_reply.elapsed() >= limit {
                        state.record(
                            "controller_unresponsive",
                            Some(controller.pid()),
                            restarts,
                        )?;
                        controller.terminate()?;
                        break;
                    }
                }
            }
            if quitting.is_some_and(|since| since.elapsed() >= Duration::from_secs(20)) {
                controller.terminate()?;
                state.record("stopped_after_timeout", None, restarts)?;
                return Ok(());
            }
            thread::sleep(Duration::from_secs(5));
        }
        restarts = restarts.saturating_add(1);
        let delay = backoff.next();
        state.record("recovery_backoff", None, restarts)?;
        thread::sleep(delay);
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;

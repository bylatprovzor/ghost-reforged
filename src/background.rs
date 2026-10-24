//! Launch and window ownership are isolated from the hosting state machine.
#[cfg(windows)]
mod native {
    use std::{
        mem::{size_of, zeroed},
        path::PathBuf,
        ptr::null,
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
        System::Threading::{
            CreateMutexW, CreateProcessW, OpenProcess, PROCESS_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, ReleaseMutex, STARTF_USESHOWWINDOW, STARTUPINFOW,
            TerminateProcess, WaitForSingleObject,
        },
        UI::WindowsAndMessaging::SW_HIDE,
    };

    pub struct Background {
        pid: u32,
        guard: Option<crate::background_window::Guard>,
        audio: Option<crate::background_audio::Guard>,
        render: Option<crate::background_render::Guard>,
        priority: Option<crate::background_priority::Guard>,
        preview: Option<crate::window_capture::Suppression>,
        process: HANDLE,
        minimize_window: bool,
    }

    pub struct HostLease(HANDLE);
    impl HostLease {
        pub fn acquire() -> Result<Self, String> {
            Self::named("Local\\GHostReborn.Hosting")
        }
        fn named(name: &str) -> Result<Self, String> {
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mutex = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
            if mutex.is_null() {
                return Err("Cannot acquire the local hosting lock.".into());
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe {
                    CloseHandle(mutex);
                }
                return Err("Another GHost Reforged controller is already running.".into());
            }
            Ok(Self(mutex))
        }
    }
    impl Drop for HostLease {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
                CloseHandle(self.0);
            }
        }
    }

    pub(crate) fn launch(
        quiet: &mut crate::background_launcher::Launch,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<Option<Background>, String> {
        let root = std::env::var_os("ProgramFiles(x86)")
            .ok_or("Warcraft III installation was not found.")?;
        let root = PathBuf::from(root);
        let launcher = root.join("Battle.net/Battle.net.exe");
        if launcher.is_file() {
            quiet.prepare(&launcher, stop, deadline)?;
            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            spawn(&launcher, "--exec=\"launch W3\"")?;
            quiet.requested();
            return Ok(None);
        }
        let path = root.join("Warcraft III/_retail_/x86_64/Warcraft III.exe");
        if !path.is_file() {
            return Err("Start Warcraft III before hosting on this installation.".into());
        }
        Ok(Some(Background::attach(spawn(
            &path,
            "-launch -windowmode windowed",
        )?)?))
    }

    pub(crate) fn spawn(path: &std::path::Path, arguments: &str) -> Result<u32, String> {
        let mut command: Vec<u16> = format!("\"{}\" {arguments}", path.display())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let executable: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let directory: Vec<u16> = path
            .parent()
            .unwrap()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut startup: STARTUPINFOW = unsafe { zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = SW_HIDE as u16;
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        if unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                0,
                null(),
                directory.as_ptr(),
                &startup,
                &mut info,
            )
        } == 0
        {
            return Err("The game launcher could not be started.".into());
        }
        unsafe {
            CloseHandle(info.hThread);
            CloseHandle(info.hProcess);
        }
        Ok(info.dwProcessId)
    }

    impl Background {
        pub fn attach(pid: u32) -> Result<Self, String> {
            let process = unsafe {
                OpenProcess(
                    PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            if process.is_null() {
                return Err("Cannot retain the Warcraft III process handle.".into());
            }
            Ok(Self {
                pid,
                guard: None,
                audio: None,
                render: None,
                priority: None,
                preview: None,
                process,
                minimize_window: true,
            })
        }
        pub fn terminate(&mut self) -> Result<(), String> {
            // The retained handle identifies this client even if Windows reuses its PID.
            if unsafe { WaitForSingleObject(self.process, 0) } == 0 {
                self.audio = None;
                self.guard = None;
                return Ok(());
            }
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            if unsafe {
                QueryFullProcessImageNameW(self.process, 0, path.as_mut_ptr(), &mut length)
            } == 0
            {
                return Err("Cannot verify the client before restarting it.".into());
            }
            let path = PathBuf::from(String::from_utf16_lossy(&path[..length as usize]));
            if !path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Warcraft III.exe"))
            {
                return Err("The retained process is not Warcraft III.".into());
            }
            // Termination must not depend on a hung window acknowledging a restore.
            // Closing the guardian pipe releases its suspension lease independently.
            self.render = None;
            self.preview = None;
            if unsafe { TerminateProcess(self.process, 0) } == 0
                || unsafe { WaitForSingleObject(self.process, 5000) } != 0
            {
                return Err("Warcraft III did not close; restart was stopped.".into());
            }
            self.guard = None;
            self.audio = None;
            Ok(())
        }
        pub(crate) fn running(&self) -> bool {
            unsafe { WaitForSingleObject(self.process, 0) != 0 }
        }
        pub fn pid(&self) -> u32 {
            self.pid
        }
        pub fn reveal(&mut self) {
            let _ = self.low_resource(false);
            self.render = None;
            self.preview = None;
            self.audio = None;
            self.guard = None;
            self.priority = None;
        }
        pub(crate) fn background_priority(&mut self, headless: bool) -> Result<(), String> {
            if !headless {
                self.priority = None;
            } else {
                if self.priority.is_none() {
                    self.priority = Some(crate::background_priority::Guard::new(self.pid)?);
                }
                if let Some(priority) = &mut self.priority {
                    priority.poll()?;
                }
            }
            Ok(())
        }
        pub fn low_resource(&mut self, enabled: bool) -> Result<(), String> {
            self.update_resources(enabled, self.minimize_window)
        }
        pub(crate) fn set_minimize_window(&mut self, enabled: bool) {
            self.minimize_window = enabled;
        }
        pub(crate) fn pause_creation(&mut self) -> Result<(), String> {
            // Authentication is complete. Native creation does not need CEF,
            // and waiting for minimization can leave software rendering busy.
            self.update_resources(true, false)
        }
        fn update_resources(&mut self, enabled: bool, minimize: bool) -> Result<(), String> {
            // CEF may spawn or replace children after the native API opens.
            // Keep tracking them during startup and sign-in as well as in a lobby.
            if let Some(priority) = &mut self.priority {
                priority.poll()?;
            }
            let result = self.apply_low_resource(enabled, minimize);
            if result.is_err() {
                // Closing the pipe releases the guardian's lease even after a lost reply.
                self.render = None;
                self.preview = None;
                if self.minimize_window
                    && let Some(guard) = &self.guard
                {
                    let _ = guard.minimize(false);
                }
            }
            result
        }
        fn apply_low_resource(&mut self, enabled: bool, minimize: bool) -> Result<(), String> {
            if enabled {
                self.hide()?;
                if minimize
                    && let Some(guard) = &self.guard
                    && !guard.try_minimize(true)?
                {
                    // Window messages may be slow on software-rendered servers.
                    // Do not begin a new suspension before minimization. An
                    // existing creation lease must keep renewing during this
                    // transition; otherwise CEF resumes after four seconds.
                    if let Some(render) = &mut self.render {
                        render.renew_pause()?;
                    }
                    return Ok(());
                }
                if self.render.is_none() {
                    self.render = Some(crate::background_render::Guard::start(self.pid)?);
                }
                if self.preview.is_none() {
                    self.preview = Some(crate::window_capture::Suppression::new(self.pid));
                }
            }
            if let Some(render) = &mut self.render {
                render.set_paused(enabled)?;
            }
            if !enabled {
                if self.minimize_window
                    && let Some(guard) = &self.guard
                {
                    guard.minimize(false)?;
                }
                self.preview = None;
            }
            Ok(())
        }
        pub fn hide(&mut self) -> Result<(), String> {
            self.hide_until(
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(2),
            )
        }
        pub(crate) fn hide_until(
            &mut self,
            stop: &AtomicBool,
            deadline: Instant,
        ) -> Result<(), String> {
            if self.audio.is_none() {
                self.audio = Some(crate::background_audio::Guard::start(self.pid)?);
            }
            if self
                .guard
                .as_ref()
                .is_some_and(|guard| guard.check().is_err())
            {
                self.guard = None;
            }
            if let Some(guard) = &self.guard {
                guard.check()?;
            } else {
                self.guard = Some(
                    crate::background_window::Guard::start_until(self.pid, stop, deadline)
                        .map_err(|error| format!("Warcraft III: {error}"))?,
                );
            }
            Ok(())
        }
    }

    impl Drop for Background {
        fn drop(&mut self) {
            self.reveal();
            unsafe {
                CloseHandle(self.process);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn slow_minimize_keeps_the_hosting_loop_available_without_suspending_the_browser() {
            use std::{
                io::{BufRead, Write},
                os::windows::process::CommandExt,
                process::{Command, Stdio},
            };
            let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("Warcraft III.exe");
            std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
            let mut child = Command::new(path)
                .args([
                    "--exact",
                    "background_window::tests::window_fixture",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("GHOST_WINDOW_TEST_FIXTURE", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                .spawn()
                .unwrap();
            let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut ready = || {
                let mut line = String::new();
                loop {
                    line.clear();
                    assert!(output.read_line(&mut line).unwrap() > 0);
                    if line.trim().starts_with("WINDOW ") {
                        break;
                    }
                }
            };
            ready();
            let mut background = Background::attach(child.id()).unwrap();
            background.hide().unwrap();
            writeln!(child.stdin.as_mut().unwrap(), "block").unwrap();
            ready();
            let began = Instant::now();
            let result = background.low_resource(true);
            let elapsed = began.elapsed();
            let unpaused = background.render.is_none();
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut minimized = false;
            while Instant::now() < deadline {
                if background
                    .guard
                    .as_ref()
                    .unwrap()
                    .try_minimize(true)
                    .unwrap_or(false)
                {
                    minimized = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // This fixture models only the native window. Browser suspension
            // and lease recovery are covered by the render-family fixtures.
            let resumed = background.low_resource(false);
            // Always dispose the native fixture, including on the red run.
            let _ = child.kill();
            let _ = child.wait();
            assert!(result.is_ok(), "{result:?}");
            assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
            assert!(unpaused, "Window readiness must precede browser suspension");
            assert!(
                minimized,
                "The pending native transition must eventually complete"
            );
            assert!(resumed.is_ok(), "{resumed:?}");
        }

        #[test]
        fn a_hung_window_cannot_prevent_terminating_the_retained_client() {
            use std::{
                io::{BufRead, Write},
                os::windows::process::CommandExt,
                process::{Command, Stdio},
            };
            let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("Warcraft III.exe");
            std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
            let mut child = Command::new(path)
                .args([
                    "--exact",
                    "background_window::tests::window_fixture",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("GHOST_WINDOW_TEST_FIXTURE", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                .spawn()
                .unwrap();
            let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut ready = || {
                let mut line = String::new();
                loop {
                    line.clear();
                    assert!(output.read_line(&mut line).unwrap() > 0);
                    if line.trim().starts_with("WINDOW ") {
                        break;
                    }
                }
            };
            ready();
            let mut background = Background::attach(child.id()).unwrap();
            background.guard = Some(crate::background_window::Guard::start(child.id()).unwrap());
            background.guard.as_ref().unwrap().minimize(true).unwrap();
            writeln!(child.stdin.as_mut().unwrap(), "stall").unwrap();
            ready();
            let began = Instant::now();
            let result = background.terminate();
            let exited = !background.running();
            // Clean up the fixture even when the regression fails.
            let _ = child.kill();
            let _ = child.wait();
            assert!(result.is_ok(), "{result:?}");
            assert!(exited);
            assert!(began.elapsed() < Duration::from_secs(7));
        }

        #[test]
        fn capture_cannot_block_the_hosting_loop() {
            let _test = crate::background_window::TEST_PRESENTATION.lock().unwrap();
            let mut background = Background::attach(std::process::id()).unwrap();
            background.hide().unwrap();
            let held = crate::window_capture::window_access();
            let started = std::time::Instant::now();
            background.hide().unwrap();
            assert!(started.elapsed() < std::time::Duration::from_millis(100));
            drop(held);
        }
        #[test]
        fn windows_host_lock_excludes_a_second_controller_and_releases() {
            let name = format!("Local\\GHostReborn.Test.{}", std::process::id());
            let first = HostLease::named(&name).unwrap();
            assert!(HostLease::named(&name).is_err());
            drop(first);
            assert!(HostLease::named(&name).is_ok());
        }
    }
}

#[cfg(windows)]
pub use crate::background_audio::Silence;
#[cfg(windows)]
pub use native::*;

#[cfg(not(windows))]
#[derive(Default)]
pub struct Silence;
#[cfg(not(windows))]
impl Silence {
    pub fn target(&mut self, _: Option<u32>) {}
    pub fn error(&self) -> Option<String> {
        None
    }
}

#[cfg(not(windows))]
pub struct Background;
#[cfg(not(windows))]
pub struct HostLease;
#[cfg(not(windows))]
impl HostLease {
    pub fn acquire() -> Result<Self, String> {
        Err("Hosting requires Windows.".into())
    }
}
#[cfg(not(windows))]
impl Background {
    pub fn attach(_: u32) -> Result<Self, String> {
        Ok(Self)
    }
    pub(crate) fn set_minimize_window(&mut self, _: bool) {}
    pub(crate) fn background_priority(&mut self, _: bool) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn running(&self) -> bool {
        false
    }
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn hide(&mut self) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn hide_until(
        &mut self,
        _: &std::sync::atomic::AtomicBool,
        _: std::time::Instant,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn reveal(&mut self) {}
    pub fn low_resource(&mut self, _: bool) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn pause_creation(&mut self) -> Result<(), String> {
        Err("Background hosting requires Windows.".into())
    }
    pub fn terminate(&mut self) -> Result<(), String> {
        Err("Restarting Warcraft III requires Windows.".into())
    }
}

/// Internal child-process entry point; call before initializing the launcher UI.
pub fn render_guardian_entry() -> bool {
    #[cfg(windows)]
    {
        crate::background_render::entry()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
#[cfg(not(windows))]
pub(crate) fn launch(
    _: &mut LauncherLaunch,
    _: &std::sync::atomic::AtomicBool,
    _: std::time::Instant,
) -> Result<Option<Background>, String> {
    Err("Background hosting requires Windows.".into())
}

#[cfg(windows)]
pub(crate) use crate::background_launcher::Launch as LauncherLaunch;
#[cfg(windows)]
pub(crate) use native::spawn as spawn_hidden;
#[cfg(not(windows))]
#[derive(Default)]
pub(crate) struct LauncherLaunch;
#[cfg(not(windows))]
impl LauncherLaunch {
    pub(crate) fn prepare_start(
        &mut self,
        _: &std::sync::atomic::AtomicBool,
        _: std::time::Instant,
    ) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn poll(
        &mut self,
        _: &std::sync::atomic::AtomicBool,
        _: std::time::Instant,
    ) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn pending(&self) -> bool {
        false
    }
    pub(crate) fn client_observed(&mut self) {}
    pub(crate) fn complete(&mut self) {}
    pub(crate) fn watch(
        &mut self,
        stop: &std::sync::atomic::AtomicBool,
        deadline: std::time::Instant,
    ) -> Result<(), String> {
        self.poll(stop, deadline)
    }
}

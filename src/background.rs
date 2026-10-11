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
        process: HANDLE,
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
                process,
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
            if unsafe { TerminateProcess(self.process, 0) } == 0
                || unsafe { WaitForSingleObject(self.process, 5000) } != 0
            {
                return Err("Warcraft III did not close; restart was stopped.".into());
            }
            self.guard = None;
            self.audio = None;
            Ok(())
        }
        pub fn pid(&self) -> u32 {
            self.pid
        }
        pub fn reveal(&mut self) {
            self.audio = None;
            self.guard = None;
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
    pub fn terminate(&mut self) -> Result<(), String> {
        Err("Restarting Warcraft III requires Windows.".into())
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
#[cfg(not(windows))]
#[derive(Default)]
pub(crate) struct LauncherLaunch;
#[cfg(not(windows))]
impl LauncherLaunch {
    pub(crate) fn poll(
        &mut self,
        _: &std::sync::atomic::AtomicBool,
        _: std::time::Instant,
    ) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn retry_if_needed(
        &mut self,
        _: bool,
        _: &std::sync::atomic::AtomicBool,
        _: std::time::Instant,
    ) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn complete(&mut self) {}
}

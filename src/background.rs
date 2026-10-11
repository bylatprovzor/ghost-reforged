//! Launch and window ownership are isolated from the hosting state machine.
#[cfg(windows)]
mod native {
    use std::{
        mem::{size_of, zeroed},
        path::PathBuf,
        ptr::null,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM},
        System::Threading::{
            CreateMutexW, CreateProcessW, OpenProcess, PROCESS_INFORMATION,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, ReleaseMutex, STARTF_USESHOWWINDOW, STARTUPINFOW,
            TerminateProcess, WaitForSingleObject,
        },
        UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
        UI::WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
            IsWindowVisible, SW_HIDE, SW_SHOWNA, SetWindowLongPtrW, ShowWindowAsync,
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    };

    const SHELL_STYLES: isize = (WS_EX_APPWINDOW | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize;

    struct HiddenWindow {
        handle: HWND,
        shell_style: isize,
        enabled: bool,
    }

    pub struct Background {
        pid: u32,
        hidden: Vec<HiddenWindow>,
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

    pub fn launch() -> Result<Option<Background>, String> {
        let root = std::env::var_os("ProgramFiles(x86)")
            .ok_or("Warcraft III installation was not found.")?;
        let root = PathBuf::from(root);
        let launcher = root.join("Battle.net/Battle.net.exe");
        if launcher.is_file() {
            spawn(&launcher, "--exec=\"launch W3\"")?;
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

    fn spawn(path: &std::path::Path, arguments: &str) -> Result<u32, String> {
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
                hidden: Vec::new(),
                process,
            })
        }
        pub fn terminate(&mut self) -> Result<(), String> {
            // The retained handle identifies this client even if Windows reuses its PID.
            if unsafe { WaitForSingleObject(self.process, 0) } == 0 {
                self.hidden.clear();
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
            self.hidden.clear();
            Ok(())
        }
        pub fn pid(&self) -> u32 {
            self.pid
        }
        pub fn reveal(&mut self) {
            let _lock = crate::window_capture::window_access();
            for saved in &self.hidden {
                let window = saved.handle;
                let mut pid = 0;
                unsafe {
                    GetWindowThreadProcessId(window, &mut pid);
                    if pid == self.pid {
                        let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
                        SetWindowLongPtrW(
                            window,
                            GWL_EXSTYLE,
                            (style & !SHELL_STYLES) | saved.shell_style,
                        );
                        EnableWindow(window, i32::from(saved.enabled));
                        ShowWindowAsync(window, SW_SHOWNA);
                    }
                }
            }
            self.hidden.clear();
        }
        pub fn hide(&mut self) {
            // Capture temporarily owns window presentation. Never stall native
            // event processing behind PrintWindow while the game starts loading.
            let Some(_lock) = crate::window_capture::try_window_access() else {
                return;
            };
            unsafe extern "system" fn window(window: HWND, param: LPARAM) -> i32 {
                let this = unsafe { &mut *(param as *mut Background) };
                let mut pid = 0;
                unsafe {
                    GetWindowThreadProcessId(window, &mut pid);
                }
                let mut title = [0u16; 128];
                let length =
                    unsafe { GetWindowTextW(window, title.as_mut_ptr(), 128) }.max(0) as usize;
                let primary =
                    String::from_utf16_lossy(&title[..length]).starts_with("Warcraft III");
                if pid == this.pid && (primary || unsafe { IsWindowVisible(window) } != 0) {
                    unsafe {
                        let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
                        if !this.hidden.iter().any(|saved| saved.handle == window) {
                            this.hidden.push(HiddenWindow {
                                handle: window,
                                shell_style: style & SHELL_STYLES,
                                enabled: IsWindowEnabled(window) != 0,
                            });
                        }
                        if IsWindowVisible(window) != 0 {
                            ShowWindowAsync(window, SW_HIDE);
                        }
                        let hidden_style = (style & !(WS_EX_APPWINDOW as isize))
                            | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize;
                        if style != hidden_style {
                            SetWindowLongPtrW(window, GWL_EXSTYLE, hidden_style);
                        }
                        if IsWindowEnabled(window) != 0 {
                            EnableWindow(window, 0);
                        }
                    }
                }
                1
            }
            unsafe {
                EnumWindows(Some(window), self as *mut Self as LPARAM);
            }
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
            let mut background = Background::attach(std::process::id()).unwrap();
            let held = crate::window_capture::window_access();
            let started = std::time::Instant::now();
            background.hide();
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

        #[test]
        fn an_already_hidden_game_window_is_tracked_and_revealed_on_stop() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let title: Vec<u16> = "Warcraft III window test\0".encode_utf16().collect();
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    0,
                    0,
                    0,
                    8,
                    8,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    null(),
                )
            };
            assert!(!window.is_null());
            let mut background = Background::attach(std::process::id()).unwrap();
            background.hide();
            let tracked = background.hidden.iter().any(|saved| saved.handle == window);
            let protected = unsafe {
                IsWindowEnabled(window) == 0
                    && GetWindowLongPtrW(window, GWL_EXSTYLE) & SHELL_STYLES
                        == (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize
            };
            background.reveal();
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
                };
                let mut message: MSG = zeroed();
                while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&message);
                }
            }
            let visible = unsafe { IsWindowVisible(window) } != 0;
            unsafe {
                DestroyWindow(window);
            }
            assert!(tracked);
            assert!(protected);
            assert!(visible);
            assert!(background.hidden.is_empty());
        }
    }
}

#[cfg(windows)]
pub use native::*;

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
    pub fn hide(&mut self) {}
    pub fn reveal(&mut self) {}
    pub fn terminate(&mut self) -> Result<(), String> {
        Err("Restarting Warcraft III requires Windows.".into())
    }
}
#[cfg(not(windows))]
pub fn launch() -> Result<Option<Background>, String> {
    Err("Background hosting requires Windows.".into())
}

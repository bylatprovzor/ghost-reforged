//! Keep presentation protection alive while the hosting socket is busy.
use std::{
    cell::RefCell,
    mem::zeroed,
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM},
    System::Threading::{
        CreateMutexW, GetCurrentThreadId, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    },
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
        WindowsAndMessaging::*,
    },
};

const SHELL: isize =
    (WS_EX_APPWINDOW | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT)
        as isize;
const STOP: u32 = WM_APP + 37;
const MARK: &[u16] = &[
    71, 72, 111, 115, 116, 46, 87, 105, 110, 100, 111, 119, 71, 117, 97, 114, 100, 0,
];

thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

#[cfg(test)]
pub(crate) static TEST_PRESENTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) struct Guard {
    thread_id: u32,
    worker: Option<thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    restore_visibility: Arc<AtomicBool>,
}

struct Lease(HANDLE);
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

impl Guard {
    #[cfg(test)]
    pub fn start(pid: u32) -> Result<Self, String> {
        Self::start_until(
            pid,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(2),
        )
    }

    pub fn start_until(
        pid: u32,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<Self, String> {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Hosting was stopped.".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let restore_visibility = Arc::new(AtomicBool::new(true));
        let restore = restore_visibility.clone();
        let worker = thread::Builder::new()
            .name("ghost-windows".into())
            .spawn(move || {
                let name: Vec<u16> = format!("Local\\GHostReforged.Windows.{pid}\0")
                    .encode_utf16()
                    .collect();
                let lease = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
                let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
                let _lease = Lease(lease);
                if lease.is_null() || exists {
                    let _ = tx.send(Err(
                        "Background windows are still being restored. Try again shortly.",
                    ));
                    return;
                }
                let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                if process.is_null() {
                    let _ = tx.send(Err("Cannot watch the game process."));
                    return;
                }
                let mut message: MSG = unsafe { zeroed() };
                unsafe {
                    PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                }
                let thread_id = unsafe { GetCurrentThreadId() };
                STATE.with(|state| {
                    *state.borrow_mut() = Some(State {
                        pid,
                        process,
                        cookie: thread_id as usize as HANDLE,
                        windows: Vec::new(),
                        capture_owned: false,
                        restore_visibility: restore,
                    })
                });
                let hooks = unsafe {
                    [
                        SetWinEventHook(
                            EVENT_OBJECT_CREATE,
                            EVENT_OBJECT_SHOW,
                            null_mut(),
                            Some(changed),
                            pid,
                            0,
                            WINEVENT_OUTOFCONTEXT,
                        ),
                        SetWinEventHook(
                            EVENT_SYSTEM_FOREGROUND,
                            EVENT_SYSTEM_FOREGROUND,
                            null_mut(),
                            Some(changed),
                            pid,
                            0,
                            WINEVENT_OUTOFCONTEXT,
                        ),
                    ]
                };
                if hooks.iter().any(|hook| hook.is_null()) {
                    for hook in hooks {
                        if !hook.is_null() {
                            unsafe {
                                UnhookWinEvent(hook);
                            }
                        }
                    }
                    STATE.with(|state| {
                        state.borrow_mut().take();
                    });
                    let _ = tx.send(Err("Cannot protect background game windows."));
                    return;
                }
                {
                    // Capture must finish restoring its temporary styles before
                    // the first protection snapshot. The caller's wait is bounded.
                    let _capture = crate::window_capture::window_access();
                    if !stopping.load(Ordering::SeqCst) {
                        with_state(|state| state.scan_locked());
                    }
                }
                let _ = tx.send(Ok(thread_id));
                // The timer only repairs presentation flags changed without a show event.
                let timer = unsafe { SetTimer(null_mut(), 0, 1000, None) };
                while !stopping.load(Ordering::SeqCst)
                    && unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0
                {
                    if message.message == STOP {
                        break;
                    }
                    if message.message == WM_TIMER {
                        let mut alive = false;
                        with_state(|state| {
                            alive = state.alive();
                            if alive {
                                state.scan();
                            }
                        });
                        if !alive {
                            break;
                        }
                    } else {
                        unsafe {
                            DispatchMessageW(&message);
                        }
                    }
                }
                unsafe {
                    if timer != 0 {
                        KillTimer(null_mut(), timer);
                    }
                    for hook in hooks {
                        UnhookWinEvent(hook);
                    }
                }
                STATE.with(|state| {
                    state.borrow_mut().take();
                });
            })
            .map_err(|_| "Cannot start background window protection.")?;
        // Cold clients can stop pumping window messages while loading. Keep the
        // same protection worker alive until startup finishes or the user stops.
        let result = loop {
            if cancelled.load(Ordering::SeqCst) {
                break Ok(Err("Hosting was stopped."));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Err(mpsc::RecvTimeoutError::Timeout);
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                result => break result,
            }
        };
        match result {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                worker: Some(worker),
                stop,
                restore_visibility,
            }),
            result => {
                stop.store(true, Ordering::SeqCst);
                if worker.is_finished() {
                    let _ = worker.join();
                }
                Err(result
                    .ok()
                    .and_then(Result::err)
                    .unwrap_or("The game window did not respond. Background hosting was stopped.")
                    .into())
            }
        }
    }

    pub fn check(&self) -> Result<(), String> {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            Err("Background window protection stopped.".into())
        } else {
            Ok(())
        }
    }

    pub fn keep_hidden(&self) {
        self.restore_visibility.store(false, Ordering::SeqCst);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe {
            PostThreadMessageW(self.thread_id, STOP, 0, 0);
        }
        // Cleanup keeps its own per-process lease until capture and native window
        // calls finish. A hung game must never block the controller's Stop action.
        if let Some(worker) = self.worker.take().filter(|worker| worker.is_finished()) {
            let _ = worker.join();
        }
    }
}

struct Saved {
    window: HWND,
    shell: isize,
    enabled: bool,
    layered: Option<(u32, u8, u32)>,
    reveal: bool,
}
struct State {
    pid: u32,
    process: HANDLE,
    cookie: HANDLE,
    windows: Vec<Saved>,
    capture_owned: bool,
    restore_visibility: Arc<AtomicBool>,
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|state| {
        // WinEvent callbacks can re-enter while a window operation dispatches messages.
        if let Ok(mut state) = state.try_borrow_mut()
            && let Some(state) = state.as_mut()
        {
            f(state);
        }
    });
}

unsafe extern "system" fn changed(
    _: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if !window.is_null() && (object == OBJID_WINDOW || event == EVENT_SYSTEM_FOREGROUND) {
        with_state(|state| {
            if event == EVENT_OBJECT_DESTROY {
                if unsafe { GetPropW(window, MARK.as_ptr()) } != state.cookie {
                    state.windows.retain(|saved| saved.window != window);
                }
            } else {
                state.protect(window);
                // Remember a suppressed show request even if the window was
                // initially created hidden. Rollback must not strand it.
                if event == EVENT_OBJECT_SHOW
                    && let Some(saved) = state
                        .windows
                        .iter_mut()
                        .find(|saved| saved.window == window)
                {
                    saved.reveal = true;
                }
            }
        });
    }
}

unsafe extern "system" fn enumerate(window: HWND, data: LPARAM) -> i32 {
    unsafe {
        (&mut *(data as *mut State)).protect(window);
    }
    1
}

pub(crate) fn protected(window: HWND) -> bool {
    (unsafe { !GetPropW(window, MARK.as_ptr()).is_null() })
        && layered_attributes(window)
            .is_some_and(|(_, alpha, flags)| alpha == 0 && flags & LWA_ALPHA != 0)
}

fn layered_attributes(window: HWND) -> Option<(u32, u8, u32)> {
    let (mut color, mut alpha, mut flags) = (0, 0, 0);
    (unsafe { GetLayeredWindowAttributes(window, &mut color, &mut alpha, &mut flags) } != 0)
        .then_some((color, alpha, flags))
}

impl State {
    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == 258 }
    }

    fn scan(&mut self) {
        if let Some(_capture) = crate::window_capture::try_window_access() {
            self.scan_locked();
        } else {
            let windows: Vec<_> = self.windows.iter().map(|saved| saved.window).collect();
            for window in windows {
                self.protect(window);
            }
        }
    }

    fn scan_locked(&mut self) {
        if self.alive() {
            self.capture_owned = true;
            self.windows.retain(|saved| unsafe {
                IsWindow(saved.window) != 0 && GetPropW(saved.window, MARK.as_ptr()) == self.cookie
            });
            unsafe {
                EnumWindows(Some(enumerate), self as *mut Self as LPARAM);
            }
            self.capture_owned = false;
        }
    }

    fn protect(&mut self, window: HWND) {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        }
        if pid != self.pid || !self.alive() || unsafe { GetAncestor(window, GA_ROOT) } != window {
            return;
        }
        let marked = unsafe { GetPropW(window, MARK.as_ptr()) } == self.cookie;
        let _capture = if !marked && !self.capture_owned {
            let Some(capture) = crate::window_capture::try_window_access() else {
                return;
            };
            Some(capture)
        } else {
            None
        };
        if !marked {
            let mut title = [0u16; 128];
            let len = unsafe { GetWindowTextW(window, title.as_mut_ptr(), 128) }.max(0) as usize;
            let primary = String::from_utf16_lossy(&title[..len]).starts_with("Warcraft III");
            if unsafe { SetPropW(window, MARK.as_ptr(), self.cookie) } == 0 {
                return;
            }
            self.windows.retain(|saved| saved.window != window);
            self.windows.push(Saved {
                window,
                shell: unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) } & SHELL,
                enabled: unsafe { IsWindowEnabled(window) } != 0,
                layered: layered_attributes(window),
                reveal: primary || unsafe { IsWindowVisible(window) } != 0,
            });
        }
        // Persistent zero opacity survives ShowWindow while the worker or preview
        // is busy. New top-level windows are protected by the process-scoped hook.
        unsafe {
            let style = GetWindowLongPtrW(window, GWL_EXSTYLE);
            let saved = self
                .windows
                .iter()
                .find(|saved| saved.window == window)
                .unwrap();
            let per_pixel = saved.shell & WS_EX_LAYERED as isize != 0 && saved.layered.is_none();
            let protected = (style & !(WS_EX_APPWINDOW as isize))
                | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT) as isize
                | if per_pixel { 0 } else { WS_EX_LAYERED as isize };
            if style != protected {
                SetWindowLongPtrW(window, GWL_EXSTYLE, protected);
            }
            // UpdateLayeredWindow owns per-pixel alpha. Do not replace its contract
            // with constant opacity, which cannot be restored from window attributes.
            if !per_pixel {
                SetLayeredWindowAttributes(window, 0, 0, LWA_ALPHA);
            } else if IsWindowVisible(window) != 0 {
                ShowWindowAsync(window, SW_HIDE);
            }
        }
        let _lock = if self.capture_owned || _capture.is_some() {
            None
        } else {
            let Some(lock) = crate::window_capture::try_window_access() else {
                return;
            };
            Some(lock)
        };
        unsafe {
            if IsWindowEnabled(window) != 0 {
                EnableWindow(window, 0);
            }
            if IsWindowVisible(window) != 0 {
                ShowWindowAsync(window, SW_HIDE);
            }
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if self.alive() {
            let _lock = crate::window_capture::window_access();
            for saved in &self.windows {
                let mut pid = 0;
                unsafe {
                    GetWindowThreadProcessId(saved.window, &mut pid);
                    if pid != self.pid || GetPropW(saved.window, MARK.as_ptr()) != self.cookie {
                        continue;
                    }
                    let style = GetWindowLongPtrW(saved.window, GWL_EXSTYLE);
                    SetWindowLongPtrW(saved.window, GWL_EXSTYLE, (style & !SHELL) | saved.shell);
                    EnableWindow(saved.window, i32::from(saved.enabled));
                    if let Some((color, alpha, flags)) = saved.layered {
                        SetLayeredWindowAttributes(saved.window, color, alpha, flags);
                    }
                    RemovePropW(saved.window, MARK.as_ptr());
                    if saved.reveal && self.restore_visibility.load(Ordering::SeqCst) {
                        ShowWindowAsync(saved.window, SW_SHOWNA);
                    }
                }
            }
        }
        unsafe {
            CloseHandle(self.process);
        }
    }
}

#[cfg(test)]
#[path = "background_window_tests.rs"]
mod tests;

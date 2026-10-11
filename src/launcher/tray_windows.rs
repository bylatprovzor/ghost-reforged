use super::Event;
use iced::futures::channel::mpsc::UnboundedSender;
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{Shell::*, WindowsAndMessaging::*},
};

const CALLBACK: u32 = WM_APP + 11;
const OPEN: usize = 1;
const QUIT: usize = 2;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const CLASS: *const u16 = windows_sys::w!("GHostReforged.Tray");

pub struct Guard {
    stop: Arc<AtomicBool>,
    thread: Arc<AtomicU32>,
}

pub fn start(events: UnboundedSender<Event>) -> Guard {
    let guard = Guard {
        stop: Arc::new(AtomicBool::new(false)),
        thread: Arc::new(AtomicU32::new(0)),
    };
    let stop = guard.stop.clone();
    let id = guard.thread.clone();
    let errors = events.clone();
    if thread::Builder::new()
        .name("ghost-tray".into())
        .spawn(move || unsafe {
            // A hidden top-level window receives TaskbarCreated after Explorer restarts.
            let module = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: module,
                lpszClassName: CLASS,
                ..zeroed()
            };
            RegisterClassW(&class);
            let icon = LoadImageW(
                module,
                std::ptr::without_provenance(1), // MAKEINTRESOURCEW(1), embedded application icon.
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                0,
            ) as HICON;
            let menu = CreatePopupMenu();
            if icon.is_null() || menu.is_null() {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
                if !menu.is_null() {
                    DestroyMenu(menu);
                }
                let _ = events.unbounded_send(Event::Available(false));
                return;
            }
            AppendMenuW(
                menu,
                MF_STRING,
                OPEN,
                windows_sys::w!("Open GHost Reforged"),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, QUIT, windows_sys::w!("Quit"));
            let mut state = State {
                events,
                icon,
                menu,
                restart: RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")),
                available: false,
            };
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                CLASS,
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                module,
                &mut state as *mut _ as _,
            );
            if !window.is_null() {
                id.store(GetCurrentThreadId(), Ordering::SeqCst);
                state.add(window);
                let mut message: MSG = zeroed();
                while !stop.load(Ordering::SeqCst)
                    && GetMessageW(&mut message, null_mut(), 0, 0) > 0
                {
                    DispatchMessageW(&message);
                }
                Shell_NotifyIconW(NIM_DELETE, &state.data(window));
                DestroyWindow(window);
            } else {
                let _ = state.events.unbounded_send(Event::Available(false));
            }
            DestroyMenu(menu);
            DestroyIcon(icon);
            let _ = state.events.unbounded_send(Event::Available(false));
        })
        .is_err()
    {
        let _ = errors.unbounded_send(Event::Available(false));
    }
    guard
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let id = self.thread.load(Ordering::SeqCst);
        if id != 0 {
            unsafe {
                PostThreadMessageW(id, WM_QUIT, 0, 0);
            }
        }
    }
}

struct State {
    events: UnboundedSender<Event>,
    icon: HICON,
    menu: HMENU,
    restart: u32,
    available: bool,
}

impl State {
    fn data(&self, window: HWND) -> NOTIFYICONDATAW {
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = window;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = self.icon;
        for (dest, unit) in data.szTip.iter_mut().zip("GHost Reforged".encode_utf16()) {
            *dest = unit;
        }
        data
    }

    fn add(&mut self, window: HWND) {
        unsafe {
            let mut data = self.data(window);
            self.available = Shell_NotifyIconW(NIM_ADD, &data) != 0;
            if self.available {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                Shell_NotifyIconW(NIM_SETVERSION, &data);
                KillTimer(window, 1);
            } else {
                SetTimer(window, 1, 2000, None);
            }
            let _ = self.events.unbounded_send(Event::Available(self.available));
        }
    }
}

unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut State;
        if !pointer.is_null() {
            // Do not hold a mutable reference across TrackPopupMenu's nested message loop.
            if message == (*pointer).restart || message == WM_TIMER {
                (*pointer).add(window);
                return 0;
            }
            if message == WM_COMMAND && l == 0 {
                if let Some(event) = menu_event(w & 0xffff) {
                    let _ = (*pointer).events.unbounded_send(event);
                }
                return 0;
            }
            if message == CALLBACK {
                match l as u32 & 0xffff {
                    NIN_SELECT | NIN_KEYSELECT => {
                        let _ = (*pointer).events.unbounded_send(Event::Open);
                    }
                    WM_CONTEXTMENU => {
                        let mut point: POINT = zeroed();
                        GetCursorPos(&mut point);
                        SetForegroundWindow(window);
                        TrackPopupMenu(
                            (*pointer).menu,
                            TPM_RIGHTBUTTON,
                            point.x,
                            point.y,
                            0,
                            window,
                            null(),
                        );
                        PostMessageW(window, WM_NULL, 0, 0);
                    }
                    _ => {}
                }
                return 0;
            }
        }
        DefWindowProcW(window, message, w, l)
    }
}

fn menu_event(command: usize) -> Option<Event> {
    match command {
        OPEN => Some(Event::Open),
        QUIT => Some(Event::Quit),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tray_windows_tests.rs"]
mod tests;

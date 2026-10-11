//! Read-only process/window observation. Never opens game memory or sends input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    Checking,
    Stopped,
    Background,
    Visible,
    Multiple,
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub state: GameState,
    pub pid: Option<u32>,
}

impl From<GameState> for Observation {
    fn from(state: GameState) -> Self {
        Self { state, pid: None }
    }
}

impl GameState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking",
            Self::Stopped => "Offline",
            Self::Background | Self::Visible | Self::Multiple => "Online",
            Self::Unavailable => "Unknown",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Checking => "Checking Warcraft III…",
            Self::Stopped => "Warcraft III is not running.",
            Self::Background => "Warcraft III is running in the background.",
            Self::Visible => "Warcraft III is running with a visible window.",
            Self::Multiple => "Multiple Warcraft III clients are running.",
            Self::Unavailable => "Warcraft III status is unavailable.",
        }
    }
}

#[cfg(windows)]
pub fn observe() -> Observation {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GWL_EXSTYLE, GetLayeredWindowAttributes, GetWindowLongPtrW,
            GetWindowThreadProcessId, IsIconic, IsWindowVisible, LWA_ALPHA, WS_EX_LAYERED,
        },
    };
    struct Windows {
        pid: u32,
        visible: bool,
    }
    unsafe extern "system" fn inspect(window: HWND, data: LPARAM) -> i32 {
        // EnumWindows calls synchronously; the stack context outlives each callback.
        let state = unsafe { &mut *(data as *mut Windows) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        }
        if pid == state.pid && unsafe { IsWindowVisible(window) != 0 && IsIconic(window) == 0 } {
            let mut color = 0;
            let mut alpha = 255;
            let mut flags = 0;
            let transparent = unsafe {
                GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_LAYERED as isize != 0
                    && GetLayeredWindowAttributes(window, &mut color, &mut alpha, &mut flags) != 0
                    && flags & LWA_ALPHA != 0
                    && alpha == 0
            };
            state.visible |= !transparent;
        }
        1
    }
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return GameState::Unavailable.into();
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ids = Vec::new();
        if Process32FirstW(snapshot, &mut entry) == 0 {
            CloseHandle(snapshot);
            return GameState::Unavailable.into();
        }
        loop {
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end])
                .eq_ignore_ascii_case("Warcraft III.exe")
            {
                ids.push(entry.th32ProcessID);
            }
            if Process32NextW(snapshot, &mut entry) == 0 {
                break;
            }
        }
        CloseHandle(snapshot);
        if ids.is_empty() {
            return GameState::Stopped.into();
        }
        if ids.len() > 1 {
            return GameState::Multiple.into();
        }
        let mut state = Windows {
            pid: ids[0],
            visible: false,
        };
        if EnumWindows(Some(inspect), &mut state as *mut Windows as LPARAM) == 0 {
            return GameState::Unavailable.into();
        }
        let state = if state.visible {
            GameState::Visible
        } else {
            GameState::Background
        };
        Observation {
            state,
            pid: Some(ids[0]),
        }
    }
}

#[cfg(not(windows))]
pub fn observe() -> Observation {
    GameState::Unavailable.into()
}

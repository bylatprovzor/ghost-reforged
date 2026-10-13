use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, LocalFree, WAIT_TIMEOUT},
    Security::{
        Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    System::{
        Console::*,
        Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        },
    },
};

pub(super) fn user_sid() -> Result<String, String> {
    process_sid(unsafe { GetCurrentProcess() })
}

fn process_sid(process: HANDLE) -> Result<String, String> {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err("Cannot identify controller user".into());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
        let mut data = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let result =
            GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), size, &mut size);
        CloseHandle(token);
        if result == 0 {
            return Err("Cannot identify controller user".into());
        }
        let user = &*data.as_ptr().cast::<TOKEN_USER>();
        let mut text = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err("Cannot identify controller user".into());
        }
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(sid)
    }
}

pub(super) fn timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Windows process creation times use 100ns ticks since 1601-01-01.
    116_444_736_000_000_000
        + (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            / 100) as u64
}

/// Retain the actual server while using its pipe. Refuse a PID reused after the
/// connection attempt, exited peers, and any principal other than the client user.
pub(super) fn authenticate_peer(
    pid: u32,
    expected: &str,
    connected_after: u64,
) -> Result<OwnedHandle, String> {
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if raw.is_null() {
        return Err("Cannot authenticate the local controller process".into());
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ok = unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) };
    let birth = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if ok == 0
        || birth > connected_after
        || process_sid(raw)? != expected
        || unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT
    {
        return Err("Local controller identity could not be verified; no command was sent".into());
    }
    Ok(process)
}

pub(super) fn descriptor() -> Result<SecurityDescriptor, String> {
    let sid = user_sid()?;
    let sddl = widestring::U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
        .map_err(|_| "Invalid controller ACL")?;
    SecurityDescriptor::deserialize(&sddl)
        .map_err(|_| "Cannot restrict the control pipe to the current user".into())
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn interrupt(kind: u32) -> i32 {
    if matches!(kind, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        INTERRUPTED.store(true, Ordering::SeqCst);
        1
    } else {
        0
    }
}
pub(super) struct Interrupts;
impl Interrupts {
    pub fn start() -> Result<Self, String> {
        INTERRUPTED.store(false, Ordering::SeqCst);
        if unsafe { SetConsoleCtrlHandler(Some(interrupt), 1) } == 0 {
            return Err("Cannot install graceful console shutdown".into());
        }
        Ok(Self)
    }
}
impl Drop for Interrupts {
    fn drop(&mut self) {
        unsafe {
            SetConsoleCtrlHandler(Some(interrupt), 0);
        }
    }
}
pub(super) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_peer_authentication_requires_same_user_live_handle_and_preconnection_birth() {
        let sid = user_sid().unwrap();
        assert!(authenticate_peer(std::process::id(), &sid, timestamp()).is_ok());
        assert!(authenticate_peer(std::process::id(), "S-1-0-0", timestamp()).is_err());
        assert!(authenticate_peer(std::process::id(), &sid, 0).is_err());
        assert!(authenticate_peer(0, &sid, timestamp()).is_err());
    }
}

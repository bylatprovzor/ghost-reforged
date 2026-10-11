//! Read-only discovery of the running game's local UI endpoint.
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("Warcraft III is not running")]
    NotRunning,
    #[error("Multiple Warcraft III clients found; close the extra client")]
    Ambiguous,
    #[error("Cannot inspect Warcraft III.")]
    Windows(std::io::Error),
    #[error("No local Warcraft III UI endpoint found")]
    NoEndpoint,
    #[cfg(not(windows))]
    #[error("Windows is required for automatic client discovery")]
    Unsupported,
}

// This value is a local session credential. Deliberately no Debug/Serialize.
pub struct Endpoint {
    pub pid: u32,
    pub ports: Vec<u16>,
    pub session: String,
    pub version: Option<String>,
}

fn session_in(bytes: &[u8]) -> Option<String> {
    for prefix in [b"?guid=".as_slice(), b"webui-socket/".as_slice()] {
        for index in memchr::memmem::find_iter(bytes, prefix) {
            let rest = &bytes[index + prefix.len()..];
            let len = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            // Require a terminator; a chunk boundary must not truncate the ID.
            if (11..=20).contains(&len) && len < rest.len() {
                return String::from_utf8(rest[..len].to_vec()).ok();
            }
        }
    }
    None
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        mem::{size_of, zeroed},
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
        },
        Networking::WinSock::AF_INET,
        System::{
            Diagnostics::{Debug::ReadProcessMemory, ToolHelp::*},
            Memory::{
                MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS,
                VirtualQueryEx,
            },
            Threading::{
                OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, QueryFullProcessImageNameW,
            },
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(crate) fn process_id() -> Result<u32, DiscoveryError> {
        unsafe {
            let raw = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if raw == INVALID_HANDLE_VALUE {
                return Err(DiscoveryError::Windows(std::io::Error::last_os_error()));
            }
            let snapshot = Handle(raw);
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ids = Vec::new();
            let mut more = Process32FirstW(snapshot.0, &mut entry);
            while more != 0 {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("Warcraft III.exe")
                {
                    ids.push(entry.th32ProcessID);
                }
                more = Process32NextW(snapshot.0, &mut entry);
            }
            match ids.as_slice() {
                [pid] => Ok(*pid),
                [] => Err(DiscoveryError::NotRunning),
                _ => Err(DiscoveryError::Ambiguous),
            }
        }
    }

    fn listener_ports(pid: u32) -> Result<Vec<u16>, DiscoveryError> {
        unsafe {
            let mut size = 0;
            GetExtendedTcpTable(
                null_mut(),
                &mut size,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            // Use aligned storage for the Windows table and tolerate a growing table.
            for _ in 0..3 {
                let mut storage = vec![0u32; (size as usize).div_ceil(4)];
                let code = GetExtendedTcpTable(
                    storage.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    AF_INET as u32,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                );
                if code == 122 {
                    continue;
                }
                if code != 0 {
                    return Err(DiscoveryError::Windows(std::io::Error::from_raw_os_error(
                        code as i32,
                    )));
                }
                let table = &*storage.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
                let rows =
                    std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
                let mut ports: Vec<_> = rows
                    .iter()
                    .filter(|r| {
                        r.dwOwningPid == pid && r.dwLocalAddr.to_ne_bytes() == [127, 0, 0, 1]
                    })
                    .map(|r| u16::from_be(r.dwLocalPort as u16))
                    .collect();
                ports.sort_unstable();
                ports.dedup();
                return Ok(ports);
            }
            Err(DiscoveryError::NoEndpoint)
        }
    }

    fn session(pid: u32) -> Result<String, DiscoveryError> {
        unsafe {
            let raw = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
            if raw.is_null() {
                return Err(DiscoveryError::Windows(std::io::Error::last_os_error()));
            }
            let handle = Handle(raw);
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut address = 0usize;
            let mut buffer = vec![0; 1024 * 1024];
            let mut total_read = 0usize;
            while Instant::now() < deadline && total_read < 2 * 1024 * 1024 * 1024usize {
                let mut info: MEMORY_BASIC_INFORMATION = zeroed();
                if VirtualQueryEx(
                    handle.0,
                    address as *const _,
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                ) == 0
                {
                    break;
                }
                let Some(end) = (info.BaseAddress as usize).checked_add(info.RegionSize) else {
                    break;
                };
                if end <= address {
                    break;
                }
                if info.State == MEM_COMMIT
                    && info.Type == MEM_PRIVATE
                    && info.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0
                {
                    let mut offset = info.BaseAddress as usize;
                    while offset < end
                        && Instant::now() < deadline
                        && total_read < 2 * 1024 * 1024 * 1024usize
                    {
                        let count = buffer.len().min(end - offset);
                        let mut read = 0;
                        ReadProcessMemory(
                            handle.0,
                            offset as *const _,
                            buffer.as_mut_ptr().cast(),
                            count,
                            &mut read,
                        );
                        total_read += count;
                        if let Some(id) = session_in(&buffer[..read]) {
                            return Ok(id);
                        }
                        // Overlap catches a URL split across blocks.
                        offset += if count > 128 { count - 128 } else { count };
                    }
                }
                address = end;
            }
            Err(DiscoveryError::NoEndpoint)
        }
    }

    pub fn discover() -> Result<Endpoint, DiscoveryError> {
        let pid = process_id()?;
        let ports = listener_ports(pid)?;
        if ports.is_empty() {
            return Err(DiscoveryError::NoEndpoint);
        }
        Ok(Endpoint {
            pid,
            ports,
            session: session(pid)?,
            version: installed_version(pid),
        })
    }

    fn installed_version(pid: u32) -> Option<String> {
        unsafe {
            let raw = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if raw.is_null() {
                return None;
            }
            let handle = Handle(raw);
            let mut path = vec![0u16; 32768];
            let mut size = path.len() as u32;
            if QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut size) == 0 {
                return None;
            }
            let executable =
                std::path::PathBuf::from(String::from_utf16_lossy(&path[..size as usize]));
            let root = executable.parent()?.parent()?.parent()?;
            super::build_version(&std::fs::read_to_string(root.join(".build.info")).ok()?)
        }
    }
}

fn build_version(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let headers: Vec<_> = lines.next()?.split('|').collect();
    let version = headers.iter().position(|s| s.starts_with("Version!"))?;
    let active = headers.iter().position(|s| s.starts_with("Active!"))?;
    lines.find_map(|line| {
        let fields: Vec<_> = line.split('|').collect();
        (fields.get(active) == Some(&"1"))
            .then(|| fields.get(version).map(|v| (*v).to_string()))
            .flatten()
    })
}

#[cfg(windows)]
pub use native::discover;
#[cfg(windows)]
pub(crate) use native::process_id;
#[cfg(not(windows))]
pub(crate) fn process_id() -> Result<u32, DiscoveryError> {
    Err(DiscoveryError::Unsupported)
}
#[cfg(not(windows))]
pub fn discover() -> Result<Endpoint, DiscoveryError> {
    Err(DiscoveryError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_both_client_url_formats() {
        assert_eq!(
            session_in(b"http://localhost/webui/index.html?guid=1234567890123\0"),
            Some("1234567890123".into())
        );
        assert_eq!(
            session_in(b"ws://localhost/webui-socket/1234567890123\0"),
            Some("1234567890123".into())
        );
    }
    #[test]
    fn rejects_truncated_or_unbounded_session_ids() {
        assert!(session_in(b"?guid=1234567890123").is_none());
        assert!(session_in(b"?guid=1234\0").is_none());
        assert!(session_in(b"?guid=123456789012345678901234\0").is_none());
    }

    #[test]
    fn selects_the_active_installed_build() {
        assert_eq!(
            build_version("Version!STRING:0|Active!DEC:1\n2.0.4|0\n3.0.0.24268|1\n"),
            Some("3.0.0.24268".into())
        );
    }

    #[test]
    fn missing_or_inactive_build_is_not_a_version_match() {
        assert_eq!(
            build_version("Version!STRING:0|Active!DEC:1\n3.0.0.24268|0\n"),
            None
        );
        assert_eq!(build_version("Version!STRING:0\n3.0.0.24268\n"), None);
    }
}

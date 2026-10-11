//! Suppress only the verified Battle.net executable during a bot-requested launch.
use crate::background_window::Guard;
use std::{
    collections::HashMap,
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, Threading::*},
};

#[derive(Default)]
pub(crate) struct Launch {
    path: Option<PathBuf>,
    guards: HashMap<u32, Guard>,
}

impl Launch {
    pub(crate) fn prepare(&mut self, path: &Path) -> Result<(), String> {
        self.path = Some(path.to_owned());
        self.poll()
    }

    pub(crate) fn poll(&mut self) -> Result<(), String> {
        let Some(expected) = &self.path else {
            return Ok(());
        };
        for pid in processes(expected)? {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.guards.entry(pid) {
                entry.insert(Guard::start(pid)?);
            }
        }
        Ok(())
    }

    pub(crate) fn complete(&mut self) {
        // Restore styles and input, but leave the launcher in its notification area.
        // Failed/cancelled launches instead restore visibility for manual sign-in.
        for guard in self.guards.values() {
            guard.keep_hidden();
        }
        self.guards.clear();
        self.path = None;
    }
}

fn processes(expected: &Path) -> Result<Vec<u32>, String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("Cannot inspect the game launcher.".into());
        }
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut result = Vec::new();
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..length])
                .eq_ignore_ascii_case("Battle.net.exe")
            {
                let process =
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !process.is_null() {
                    let mut path = [0u16; 32768];
                    let mut size = path.len() as u32;
                    if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) != 0
                        && Path::new(&String::from_utf16_lossy(&path[..size as usize])) == expected
                    {
                        result.push(entry.th32ProcessID);
                    }
                    CloseHandle(process);
                }
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        Ok(result)
    }
}

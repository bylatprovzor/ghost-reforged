use super::{SendFilter, stub};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    mem::{size_of, zeroed},
    path::PathBuf,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory},
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                TH32CS_SNAPMODULE,
            },
        },
        Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE,
            VirtualAllocEx, VirtualFreeEx, VirtualProtectEx,
        },
        Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
            PROCESS_VM_WRITE,
        },
    },
};

mod build;
const SEND: usize = 7;
const PAGE: usize = 4096;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(crate) struct NativeFilter {
    process: Handle,
    table: usize,
    methods: [usize; 9],
    allocation: Option<usize>,
    published: bool,
    active: bool,
    previous_count: u64,
    protection_to_restore: Option<u32>,
}

impl NativeFilter {
    pub(super) fn open(pid: u32) -> Result<Self, String> {
        if !cfg!(target_arch = "x86_64") {
            return Err("Blank host names require 64-bit Windows.".into());
        }
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION
                    | PROCESS_VM_OPERATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE,
                0,
                pid,
            )
        };
        if process.is_null() {
            return Err("Cannot access the game's host profile.".into());
        }
        let process = Handle(process);
        let modules = modules(pid)?;
        let exe = unique_module(&modules, "Warcraft III.exe")?;
        let sdk = unique_module(&modules, "ClientSdk.dll")?;
        let build = build::for_hashes(&file_hash(&exe.path)?, &file_hash(&sdk.path)?)
            .ok_or("This Warcraft III build has not been verified for blank host names.")?;
        if sdk.size < build.table + build.methods.len() * 8 {
            return Err("Unsupported Warcraft III SDK layout.".into());
        }
        let filter = Self {
            process,
            table: sdk.base + build.table,
            methods: build.methods.map(|rva| sdk.base + rva),
            allocation: None,
            published: false,
            active: false,
            previous_count: 0,
            protection_to_restore: None,
        };
        filter.verify_table()?;
        Ok(filter)
    }

    fn read<const N: usize>(&self, address: usize) -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        let mut count = 0;
        if unsafe {
            ReadProcessMemory(
                self.process.0,
                address as *const _,
                bytes.as_mut_ptr().cast(),
                N,
                &mut count,
            )
        } == 0
            || count != N
        {
            return Err("Cannot read the game's host profile state.".into());
        }
        Ok(bytes)
    }

    fn pointer(&self, address: usize) -> Result<usize, String> {
        Ok(u64::from_le_bytes(self.read(address)?) as usize)
    }

    fn write(&self, address: usize, bytes: &[u8]) -> Result<(), String> {
        let mut count = 0;
        if unsafe {
            WriteProcessMemory(
                self.process.0,
                address as *const _,
                bytes.as_ptr().cast(),
                bytes.len(),
                &mut count,
            )
        } == 0
            || count != bytes.len()
        {
            return Err("Cannot update the game's host profile filter.".into());
        }
        Ok(())
    }

    fn verify_table(&self) -> Result<(), String> {
        for (index, expected) in self.methods.iter().enumerate() {
            if self.pointer(self.table + index * 8)? != *expected {
                return Err(
                    "Warcraft III SDK differs from the verified build. Hosting stopped.".into(),
                );
            }
        }
        Ok(())
    }

    fn allocate(&mut self) -> Result<usize, String> {
        if let Some(address) = self.allocation {
            return Ok(address);
        }
        let address = unsafe {
            VirtualAllocEx(
                self.process.0,
                std::ptr::null(),
                PAGE * 2,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        } as usize;
        if address == 0 {
            return Err("Cannot allocate the host profile filter.".into());
        }
        let result = self.prepare_allocation(address);
        if result.is_err() {
            unsafe {
                VirtualFreeEx(self.process.0, address as *mut _, 0, MEM_RELEASE);
            }
        } else {
            self.allocation = Some(address);
        }
        result.map(|()| address)
    }

    fn prepare_allocation(&self, address: usize) -> Result<(), String> {
        let code = stub::machine_code(self.methods[SEND], address + PAGE);
        self.write(address, &code)?;
        // The counter remains writable; only the separate stub page is executable.
        let mut previous = 0;
        if unsafe {
            VirtualProtectEx(
                self.process.0,
                address as *const _,
                PAGE,
                PAGE_EXECUTE_READ,
                &mut previous,
            )
        } == 0
            || unsafe { FlushInstructionCache(self.process.0, address as *const _, code.len()) }
                == 0
        {
            return Err("Cannot prepare the host profile filter.".into());
        }
        Ok(())
    }

    fn restore_protection(&mut self) -> Result<(), String> {
        if let Some(previous) = self.protection_to_restore {
            let mut ignored = 0;
            if unsafe {
                VirtualProtectEx(
                    self.process.0,
                    (self.table + SEND * 8) as *const _,
                    8,
                    previous,
                    &mut ignored,
                )
            } == 0
            {
                return Err("Cannot restore Warcraft III SDK page protection.".into());
            }
            self.protection_to_restore = None;
        }
        Ok(())
    }

    fn exchange(&mut self, expected: usize, replacement: usize) -> Result<(), String> {
        self.restore_protection()?;
        let slot = self.table + SEND * 8;
        if self.pointer(slot)? != expected {
            return Err("Warcraft III SDK send method changed.".into());
        }
        let mut previous = 0;
        if unsafe {
            VirtualProtectEx(
                self.process.0,
                slot as *const _,
                8,
                PAGE_READWRITE,
                &mut previous,
            )
        } == 0
        {
            return Err("Cannot prepare the Warcraft III SDK send slot.".into());
        }
        self.protection_to_restore = Some(previous);
        let result = (|| {
            if self.pointer(slot)? != expected {
                return Err("Warcraft III SDK send method changed.".into());
            }
            self.write(slot, &(replacement as u64).to_le_bytes())?;
            if self.pointer(slot)? != replacement {
                return Err("Host profile filter verification failed.".into());
            }
            Ok(())
        })();
        let protection = self.restore_protection();
        result.and(protection)
    }
}

impl SendFilter for NativeFilter {
    fn install(&mut self) -> Result<(), String> {
        self.restore()?;
        self.verify_table()?;
        let address = self.allocate()?;
        self.previous_count = u64::from_le_bytes(self.read(address + PAGE)?);
        // From this point an in-flight call may retain the address, even if readback fails.
        self.published = true;
        self.active = true;
        self.exchange(self.methods[SEND], address)
    }

    fn count(&self) -> Result<u64, String> {
        let address = self
            .allocation
            .ok_or("Host profile filter was not prepared.")?;
        Ok(u64::from_le_bytes(self.read(address + PAGE)?).wrapping_sub(self.previous_count))
    }

    fn restore(&mut self) -> Result<(), String> {
        if self.active {
            let current = self.pointer(self.table + SEND * 8)?;
            if Some(current) == self.allocation {
                self.exchange(current, self.methods[SEND])?;
            } else if current != self.methods[SEND] {
                return Err("Warcraft III SDK send method changed; restoration refused.".into());
            }
            self.active = false;
        }
        self.restore_protection()
    }
}

impl Drop for NativeFilter {
    fn drop(&mut self) {
        let _ = self.restore();
        // Retain one bounded 8 KiB allocation per connection until game exit:
        // an already dispatched call can still be executing after slot restoration.
        if !self.published
            && let Some(address) = self.allocation
        {
            unsafe {
                VirtualFreeEx(self.process.0, address as *mut _, 0, MEM_RELEASE);
            }
        }
    }
}

struct Module {
    name: String,
    path: PathBuf,
    base: usize,
    size: usize,
}

fn modules(pid: u32) -> Result<Vec<Module>, String> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) };
    if raw == INVALID_HANDLE_VALUE {
        return Err("Cannot inspect the Warcraft III modules.".into());
    }
    let snapshot = Handle(raw);
    let mut entry: MODULEENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
    let mut more = unsafe { Module32FirstW(snapshot.0, &mut entry) };
    let mut result = Vec::new();
    let decode = |s: &[u16]| {
        String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())])
    };
    while more != 0 {
        result.push(Module {
            name: decode(&entry.szModule),
            path: decode(&entry.szExePath).into(),
            base: entry.modBaseAddr as usize,
            size: entry.modBaseSize as usize,
        });
        more = unsafe { Module32NextW(snapshot.0, &mut entry) };
    }
    Ok(result)
}

fn unique_module<'a>(modules: &'a [Module], name: &str) -> Result<&'a Module, String> {
    let mut matches = modules.iter().filter(|m| m.name.eq_ignore_ascii_case(name));
    let first = matches
        .next()
        .ok_or("The required Warcraft III module is not loaded.")?;
    if matches.next().is_some() {
        return Err("Ambiguous Warcraft III modules.".into());
    }
    Ok(first)
}

fn file_hash(path: &std::path::Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "Cannot verify the Warcraft III build.")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Cannot verify the Warcraft III build.")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests;

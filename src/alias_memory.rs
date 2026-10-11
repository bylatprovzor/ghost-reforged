use std::{
    mem::{size_of, zeroed},
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory},
        Memory::{
            MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQueryEx,
        },
        Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
            PROCESS_VM_WRITE,
        },
    },
};

pub struct Memory(HANDLE);
pub struct Patch {
    address: usize,
    before: [u8; 40],
    after: [u8; 40],
}

impl Memory {
    pub fn open(pid: u32) -> Result<Self, String> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE
                    | PROCESS_VM_OPERATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            Err("Cannot access the game's host-name data.".into())
        } else {
            Ok(Self(handle))
        }
    }

    fn read(&self, address: usize, buffer: &mut [u8]) -> bool {
        let mut read = 0;
        unsafe {
            ReadProcessMemory(
                self.0,
                address as *const _,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut read,
            ) != 0
                && read == buffer.len()
        }
    }

    fn ordinary_data(&self, address: usize, length: usize) -> bool {
        let mut region: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                self.0,
                address as *const _,
                &mut region,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return false;
        }
        region.State == MEM_COMMIT
            && region.Type == MEM_PRIVATE
            && region.Protect == PAGE_READWRITE
            && address
                .checked_add(length)
                .zip((region.BaseAddress as usize).checked_add(region.RegionSize))
                .is_some_and(|(end, limit)| end <= limit)
    }

    pub fn candidates(&self, name: &[u8], cancelled: &AtomicBool) -> Result<Vec<usize>, String> {
        self.scan(
            name,
            0..usize::MAX,
            Duration::from_secs(12),
            2 * 1024 * 1024 * 1024,
            cancelled,
        )
    }

    pub fn nearby_candidates(
        &self,
        name: &[u8],
        address: usize,
        cancelled: &AtomicBool,
    ) -> Result<Vec<usize>, String> {
        const RADIUS: usize = 1024 * 1024;
        self.scan(
            name,
            address.saturating_sub(RADIUS)..address.saturating_add(RADIUS),
            Duration::from_millis(100),
            2 * RADIUS + 4096,
            cancelled,
        )
    }

    fn scan(
        &self,
        name: &[u8],
        range: Range<usize>,
        duration: Duration,
        byte_limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Vec<usize>, String> {
        if !(1..=15).contains(&name.len()) {
            return Err("This account name uses an unsupported native string layout.".into());
        }
        let deadline = Instant::now() + duration;
        let mut address = range.start;
        let mut total = 0usize;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut found = Vec::new();
        while address < range.end
            && Instant::now() < deadline
            && total < byte_limit
            && !cancelled.load(Ordering::SeqCst)
        {
            let mut region: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
            if unsafe {
                VirtualQueryEx(
                    self.0,
                    address as *const _,
                    &mut region,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            } == 0
            {
                break;
            }
            let Some(end) = (region.BaseAddress as usize).checked_add(region.RegionSize) else {
                break;
            };
            if end <= address {
                break;
            }
            if region.State == MEM_COMMIT
                && region.Type == MEM_PRIVATE
                && region.Protect == PAGE_READWRITE
            {
                let mut cursor = (region.BaseAddress as usize).max(range.start);
                let scan_end = end.min(range.end);
                while cursor < scan_end
                    && Instant::now() < deadline
                    && total < byte_limit
                    && !cancelled.load(Ordering::SeqCst)
                {
                    let count = buffer.len().min(scan_end - cursor);
                    if self.read(cursor, &mut buffer[..count]) {
                        for offset in memchr::memmem::find_iter(&buffer[..count], name) {
                            let data = cursor + offset;
                            if data >= 24 && data.is_multiple_of(8) {
                                let mut object = [0u8; 40];
                                if self.read(data - 24, &mut object)
                                    && valid_object(&object, data, name)
                                {
                                    found.push(data - 24);
                                }
                            }
                        }
                    }
                    total += count;
                    cursor += if count > 64 { count - 64 } else { count };
                }
            }
            address = end;
        }
        found.sort_unstable();
        found.dedup();
        // Candidate count is not ambiguity: Alias verifies each object against
        // a fresh native lobby snapshot and bounds the overall attempt by time.
        Ok(found)
    }

    pub fn apply(
        &self,
        address: usize,
        original: &[u8],
        alias: &[u8],
    ) -> Result<Option<Patch>, String> {
        let mut before = [0u8; 40];
        if !self.ordinary_data(address, 40)
            || !self.read(address, &mut before)
            || !valid_object(&before, address + 24, original)
        {
            return Ok(None);
        }
        let mut after = before;
        after[8..16].copy_from_slice(&(alias.len() as u64).to_le_bytes());
        after[24..].fill(0);
        after[24..24 + alias.len()].copy_from_slice(alias);
        if !self.compare_write(address, &before, &after) {
            return Err("Host-name data changed before the update could be verified.".into());
        }
        Ok(Some(Patch {
            address,
            before,
            after,
        }))
    }

    fn compare_write(&self, address: usize, expected: &[u8; 40], replacement: &[u8; 40]) -> bool {
        let mut current = [0u8; 40];
        if !self.ordinary_data(address, 40)
            || !self.read(address, &mut current)
            || current != *expected
        {
            return false;
        }
        let mut written = 0;
        let success = unsafe {
            WriteProcessMemory(
                self.0,
                address as *const _,
                replacement.as_ptr().cast(),
                40,
                &mut written,
            ) != 0
        };
        success && written == 40 && self.read(address, &mut current) && current == *replacement
    }

    pub fn restore(&self, patch: Patch) {
        // A freed or changed object must never be overwritten with saved bytes.
        self.compare_write(patch.address, &patch.after, &patch.before);
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn valid_object(bytes: &[u8; 40], data: usize, name: &[u8]) -> bool {
    (1..=15).contains(&name.len())
        && u64::from_le_bytes(bytes[0..8].try_into().unwrap()) == data as u64
        && u64::from_le_bytes(bytes[8..16].try_into().unwrap()) == name.len() as u64
        && u64::from_le_bytes(bytes[16..24].try_into().unwrap()) == 0x800000000000000f
        && bytes[24..24 + name.len()] == *name
        && bytes[24 + name.len()] == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn many_valid_name_copies_do_not_prevent_native_candidate_discovery() {
        #[derive(Clone, Copy)]
        #[repr(C, align(8))]
        struct Inline([u8; 40]);
        let mut objects = Box::new([Inline([0; 40]); 96]);
        let name = b"AliasScanTest";
        let mut addresses = Vec::new();
        for object in objects.iter_mut() {
            let address = object.0.as_ptr() as usize;
            addresses.push(address);
            object.0[..8].copy_from_slice(&((address + 24) as u64).to_le_bytes());
            object.0[8..16].copy_from_slice(&(name.len() as u64).to_le_bytes());
            object.0[16..24].copy_from_slice(&0x800000000000000fu64.to_le_bytes());
            object.0[24..24 + name.len()].copy_from_slice(name);
        }
        let memory = Memory::open(std::process::id()).unwrap();
        let found = memory
            .nearby_candidates(name, addresses[0], &AtomicBool::new(false))
            .unwrap();
        assert!(addresses.iter().all(|address| found.contains(address)));
    }
    #[test]
    fn windows_name_write_restores_only_an_unchanged_object() {
        #[repr(C, align(8))]
        struct Inline(std::cell::UnsafeCell<[u8; 40]>);
        let allocation = Box::new(Inline(std::cell::UnsafeCell::new([0; 40])));
        let address = allocation.0.get() as usize;
        let mut original = [0u8; 40];
        original[..8].copy_from_slice(&((address + 24) as u64).to_le_bytes());
        original[8..16].copy_from_slice(&5u64.to_le_bytes());
        original[16..24].copy_from_slice(&0x800000000000000fu64.to_le_bytes());
        original[24..29].copy_from_slice(b"Owner");
        unsafe {
            *allocation.0.get() = original;
        }
        let memory = Memory::open(std::process::id()).unwrap();
        let patch = memory.apply(address, b"Owner", b"Ghost").unwrap().unwrap();
        let changed = unsafe { *allocation.0.get() };
        assert_eq!(&changed[24..29], b"Ghost");
        memory.restore(patch);
        assert_eq!(unsafe { *allocation.0.get() }, original);
        let patch = memory.apply(address, b"Owner", b"Ghost").unwrap().unwrap();
        unsafe {
            (*allocation.0.get())[24] = b'X';
        }
        memory.restore(patch);
        assert_eq!(unsafe { (*allocation.0.get())[24] }, b'X');
    }
    #[test]
    fn native_layout_requires_exact_pointer_length_capacity_and_terminator() {
        let mut value = [0u8; 40];
        value[..8].copy_from_slice(&0x1018u64.to_le_bytes());
        value[8..16].copy_from_slice(&5u64.to_le_bytes());
        value[16..24].copy_from_slice(&0x800000000000000fu64.to_le_bytes());
        value[24..29].copy_from_slice(b"Owner");
        assert!(valid_object(&value, 0x1018, b"Owner"));
        for index in [0, 8, 16, 24, 29] {
            let mut wrong = value;
            wrong[index] ^= 1;
            assert!(!valid_object(&wrong, 0x1018, b"Owner"));
        }
    }
}

use super::*;
use std::cell::RefCell;
use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, PAGE_READONLY, VirtualQueryEx};

thread_local! {
    static CALLS: RefCell<Vec<(usize, Vec<u8>, u32)>> = const { RefCell::new(Vec::new()) };
}
type Send = unsafe extern "system" fn(usize, *const u8, u32) -> u32;
unsafe extern "system" fn forward(this: usize, data: *const u8, length: u32) -> u32 {
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, length as usize) }.to_vec()
    };
    CALLS.with(|calls| calls.borrow_mut().push((this, bytes, length)));
    73
}

struct Fixture {
    filter: Option<NativeFilter>,
    table: usize,
}
impl Fixture {
    fn new() -> Self {
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_INFORMATION
                    | PROCESS_VM_OPERATION
                    | PROCESS_VM_READ
                    | PROCESS_VM_WRITE,
                0,
                std::process::id(),
            )
        };
        assert!(!process.is_null());
        let table = unsafe {
            VirtualAllocEx(
                process,
                std::ptr::null(),
                PAGE,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        } as usize;
        assert_ne!(table, 0);
        let filter = NativeFilter {
            process: Handle(process),
            table,
            methods: [forward as *const () as usize; 9],
            allocation: None,
            published: false,
            active: false,
            previous_count: 0,
            protection_to_restore: None,
        };
        for (index, method) in filter.methods.iter().enumerate() {
            filter
                .write(table + index * 8, &(*method as u64).to_le_bytes())
                .unwrap();
        }
        let mut previous = 0;
        assert_ne!(
            unsafe {
                VirtualProtectEx(
                    process,
                    table as *const _,
                    PAGE,
                    PAGE_READONLY,
                    &mut previous,
                )
            },
            0
        );
        Self {
            filter: Some(filter),
            table,
        }
    }
    fn filter(&mut self) -> &mut NativeFilter {
        self.filter.as_mut().unwrap()
    }
    fn send(&self, data: &[u8]) -> u32 {
        let address = self
            .filter
            .as_ref()
            .unwrap()
            .pointer(self.table + SEND * 8)
            .unwrap();
        let send: Send = unsafe { std::mem::transmute(address) };
        unsafe { send(0x1234, data.as_ptr(), data.len() as u32) }
    }
    fn protection(&self) -> u32 {
        let filter = self.filter.as_ref().unwrap();
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        assert_ne!(
            unsafe {
                VirtualQueryEx(
                    filter.process.0,
                    self.table as *const _,
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            },
            0
        );
        info.Protect
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let filter = self.filter.take().unwrap();
        let allocation = filter.allocation;
        drop(filter);
        // This fixture has no concurrent callers; production must retain the stub.
        let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
        unsafe {
            VirtualFreeEx(process, self.table as *mut _, 0, MEM_RELEASE);
            if let Some(address) = allocation {
                VirtualFreeEx(process, address as *mut _, 0, MEM_RELEASE);
            }
        }
    }
}

#[test]
fn native_send_filter_drops_only_exact_profiles_and_preserves_all_other_calls() {
    let mut fixture = Fixture::new();
    CALLS.with(|c| c.borrow_mut().clear());
    let profile = [0xf7, 0x59, 12, 0, 3, 3, 0, 0, 0, 8, 1, 0];
    assert_eq!(fixture.send(&profile), 73);
    CALLS.with(|c| c.borrow_mut().clear());
    let mut allocation = None;
    for _ in 0..3 {
        fixture.filter().install().unwrap();
        assert_eq!(fixture.protection(), PAGE_READONLY);
        assert_eq!(
            *allocation.get_or_insert(fixture.filter().allocation),
            fixture.filter().allocation
        );
        assert_eq!(fixture.send(&profile), 1);
        assert_eq!(fixture.filter().count().unwrap(), 1);
        CALLS.with(|c| assert!(c.borrow().is_empty()));
        let mut cases = vec![profile[..8].to_vec(), vec![]];
        for (offset, byte) in [(0, 0), (1, 0x1e), (2, 13), (4, 4), (5, 4)] {
            let mut changed = profile.to_vec();
            changed[offset] = byte;
            cases.push(changed);
        }
        for data in &cases {
            assert_eq!(fixture.send(data), 73);
            CALLS.with(|c| {
                assert_eq!(
                    c.borrow().last().unwrap(),
                    &(0x1234, data.clone(), data.len() as u32)
                )
            });
        }
        let send: Send = unsafe { std::mem::transmute(fixture.filter().allocation.unwrap()) };
        assert_eq!(unsafe { send(0x5678, std::ptr::null(), 9) }, 73);
        CALLS.with(|c| assert_eq!(c.borrow().last().unwrap(), &(0x5678, vec![], 9)));
        assert_eq!(fixture.filter().count().unwrap(), 1);
        fixture.filter().restore().unwrap();
        assert_eq!(fixture.protection(), PAGE_READONLY);
        assert_eq!(
            fixture.send(&profile),
            73,
            "Publication resumes after restoration"
        );
        CALLS.with(|c| c.borrow_mut().clear());
    }
}

#[test]
fn native_restore_refuses_to_overwrite_an_unrelated_send_pointer() {
    let mut fixture = Fixture::new();
    fixture.filter().install().unwrap();
    let address = fixture.filter().allocation.unwrap();
    let original = fixture.filter().methods[SEND];
    fixture.filter().exchange(address, original + 1).unwrap();
    assert!(fixture.filter().restore().is_err());
    let slot = fixture.table + SEND * 8;
    assert_eq!(fixture.filter().pointer(slot).unwrap(), original + 1);
    fixture.filter().exchange(original + 1, address).unwrap();
    fixture.filter().restore().unwrap();
    assert_eq!(fixture.protection(), PAGE_READONLY);
}

#[test]
fn a_changed_method_table_is_rejected_before_installation() {
    let mut fixture = Fixture::new();
    fixture.filter().methods[0] += 1;
    assert!(fixture.filter().install().is_err());
    assert!(fixture.filter().allocation.is_none());
    assert_eq!(fixture.protection(), PAGE_READONLY);
}

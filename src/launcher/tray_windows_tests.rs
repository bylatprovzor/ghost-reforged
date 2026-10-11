use super::*;
use std::time::{Duration, Instant};

fn receive(rx: &mut iced::futures::channel::mpsc::UnboundedReceiver<Event>) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = rx.try_recv() {
            return event;
        }
        assert!(Instant::now() < until, "Native tray did not respond");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_tray_registers_receives_clicks_recovers_and_removes_its_icon() {
    let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
    let guard = start(tx);
    assert_eq!(receive(&mut rx), Event::Available(true));
    unsafe {
        unsafe extern "system" fn find(window: HWND, output: LPARAM) -> i32 {
            unsafe {
                *(output as *mut HWND) = window;
            }
            0
        }
        let mut window: HWND = null_mut();
        EnumThreadWindows(
            guard.thread.load(Ordering::SeqCst),
            Some(find),
            &mut window as *mut _ as LPARAM,
        );
        assert!(!window.is_null());
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        assert_eq!(pid, std::process::id());
        SendMessageW(window, CALLBACK, 0, NIN_SELECT as isize);
        assert_eq!(receive(&mut rx), Event::Open);
        SendMessageW(window, CALLBACK, 0, NIN_KEYSELECT as isize);
        assert_eq!(receive(&mut rx), Event::Open);
        SendMessageW(window, WM_COMMAND, QUIT, 0);
        assert_eq!(receive(&mut rx), Event::Quit);
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            ..zeroed()
        };
        assert_ne!(Shell_NotifyIconW(NIM_DELETE, &data), 0);
        SendMessageW(
            window,
            RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")),
            0,
            0,
        );
        assert_eq!(receive(&mut rx), Event::Available(true));
        // Geometry can be unavailable while the notification area is collapsed.
        assert_ne!(Shell_NotifyIconW(NIM_MODIFY, &data), 0);
        drop(guard);
        let until = Instant::now() + Duration::from_secs(5);
        while IsWindow(window) != 0 {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(Shell_NotifyIconW(NIM_MODIFY, &data), 0);
    }
    assert_eq!(menu_event(QUIT), Some(Event::Quit));
    assert_eq!(menu_event(OPEN), Some(Event::Open));
    assert_eq!(menu_event(0), None);
}

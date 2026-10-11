use super::{Frame, window_access};
use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT},
    Graphics::Gdi::*,
    Storage::Xps::PrintWindow,
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        QueryFullProcessImageNameW, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::*,
};

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct WindowSearch {
    pid: u32,
    windows: Vec<HWND>,
}

unsafe extern "system" fn find_window(window: HWND, data: LPARAM) -> i32 {
    let search = unsafe { &mut *(data as *mut WindowSearch) };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    if pid == search.pid {
        let mut title = [0u16; 128];
        let len = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) }.max(0)
            as usize;
        if String::from_utf16_lossy(&title[..len]).starts_with("Warcraft III") {
            search.windows.push(window);
        }
    }
    1
}

struct RenderWindow {
    window: HWND,
    pid: u32,
    process: Process,
    original_style: isize,
    applied_style: isize,
    rect: RECT,
    restore: bool,
}

impl RenderWindow {
    fn open(pid: u32) -> Result<Self, String> {
        let process = Process(unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        });
        if process.0.is_null() {
            return Err("Warcraft III is unavailable.".into());
        }
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0
            || !std::path::Path::new(&String::from_utf16_lossy(&path[..length as usize]))
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("Warcraft III.exe"))
        {
            return Err("The preview target is not Warcraft III.".into());
        }
        let mut search = WindowSearch {
            pid,
            windows: Vec::new(),
        };
        unsafe {
            EnumWindows(Some(find_window), &mut search as *mut _ as LPARAM);
        }
        let [window] = search.windows.as_slice() else {
            return Err("Waiting for the game window…".into());
        };
        if unsafe { IsIconic(*window) != 0 || IsHungAppWindow(*window) != 0 } {
            return Err("The game window is not ready.".into());
        }
        let mut rect = unsafe { zeroed() };
        if unsafe { GetWindowRect(*window, &mut rect) } == 0 {
            return Err("Window bounds are unavailable.".into());
        }
        let style = unsafe { GetWindowLongPtrW(*window, GWL_EXSTYLE) };
        let mut render = Self {
            window: *window,
            pid,
            process,
            original_style: style,
            applied_style: style,
            rect,
            restore: false,
        };
        if unsafe { IsWindowVisible(*window) } == 0 {
            // A hidden DirectX window returns a black PrintWindow image. Let it
            // render briefly with zero opacity, no activation and no hit testing.
            // Existing layered windows have their own presentation contract.
            if style & WS_EX_LAYERED as isize != 0 {
                return Err("The game window uses an unsupported presentation mode.".into());
            }
            render.applied_style =
                style | (WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT) as isize;
            render.restore = true;
            unsafe {
                SetWindowLongPtrW(*window, GWL_EXSTYLE, render.applied_style);
                if SetLayeredWindowAttributes(*window, 0, 0, LWA_ALPHA) == 0 {
                    return Err("The background preview could not be prepared.".into());
                }
                if SetWindowPos(
                    *window,
                    null_mut(),
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                ) == 0
                {
                    return Err("The background preview could not be rendered.".into());
                }
            }
            thread::sleep(Duration::from_millis(350));
        }
        Ok(render)
    }
}

impl Drop for RenderWindow {
    fn drop(&mut self) {
        if !self.restore || unsafe { WaitForSingleObject(self.process.0, 0) } != 258 {
            return;
        }
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(self.window, &mut pid);
            if pid == self.pid && GetWindowLongPtrW(self.window, GWL_EXSTYLE) == self.applied_style
            {
                ShowWindow(self.window, SW_HIDE);
                SetWindowPos(
                    self.window,
                    null_mut(),
                    self.rect.left,
                    self.rect.top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                SetWindowLongPtrW(self.window, GWL_EXSTYLE, self.original_style);
            }
        }
    }
}

struct Bitmap {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
    width: i32,
    height: i32,
}

impl Bitmap {
    fn new(width: i32, height: i32) -> Result<Self, String> {
        let dc = unsafe { CreateCompatibleDC(null_mut()) };
        if dc.is_null() {
            return Err("Preview drawing surface is unavailable.".into());
        }
        let mut info: BITMAPINFO = unsafe { zeroed() };
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0) };
        if bitmap.is_null() {
            unsafe {
                DeleteDC(dc);
            }
            return Err("Preview bitmap is unavailable.".into());
        }
        let previous = unsafe { SelectObject(dc, bitmap) };
        Ok(Self {
            dc,
            bitmap,
            previous,
            pixels: pixels.cast(),
            width,
            height,
        })
    }

    fn frame(&self) -> Frame {
        unsafe {
            GdiFlush();
        }
        let mut rgba = unsafe {
            std::slice::from_raw_parts(self.pixels, (self.width * self.height * 4) as usize)
        }
        .to_vec();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Frame {
            width: self.width as u32,
            height: self.height as u32,
            rgba,
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub fn capture(pid: u32, max_width: i32) -> Result<Frame, String> {
    let _lock = window_access();
    let window = RenderWindow::open(pid)?;
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetClientRect(window.window, &mut rect) } == 0 {
        return Err("The game window closed.".into());
    }
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if !(100..=7680).contains(&width) || !(100..=4320).contains(&height) {
        return Err("Unsupported game window dimensions.".into());
    }
    let full = Bitmap::new(width, height)?;
    if unsafe { PrintWindow(window.window, full.dc, 3) } == 0 {
        return Err("The game did not provide a preview.".into());
    }
    let thumb_width = width.min(max_width);
    let thumb_height = (height * thumb_width / width).max(1);
    let thumbnail = Bitmap::new(thumb_width, thumb_height)?;
    unsafe {
        SetStretchBltMode(thumbnail.dc, HALFTONE);
        SetBrushOrgEx(thumbnail.dc, 0, 0, null_mut());
        if StretchBlt(
            thumbnail.dc,
            0,
            0,
            thumb_width,
            thumb_height,
            full.dc,
            0,
            0,
            width,
            height,
            SRCCOPY,
        ) == 0
        {
            return Err("The preview could not be resized.".into());
        }
    }
    let frame = thumbnail.frame();
    if !frame
        .rgba
        .chunks_exact(4)
        .any(|p| p[0].abs_diff(p[1]) > 8 || p[1].abs_diff(p[2]) > 8)
    {
        return Err("Waiting for the game to render…".into());
    }
    Ok(frame)
}

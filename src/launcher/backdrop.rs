use iced::widget::image;
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::Duration,
};

const FRAME_INTERVAL: Duration = Duration::from_micros(83_334);

pub struct Backdrop {
    pub frame: Option<image::Handle>,
    requests: Option<SyncSender<()>>,
    decoder_thread: Option<thread::JoinHandle<()>>,
    frames: Receiver<Option<image::Handle>>,
    waiting: bool,
    failed: bool,
}

impl Backdrop {
    pub fn new() -> Self {
        let (requests, rx) = mpsc::sync_channel(1);
        let (tx, frames) = mpsc::sync_channel(1);
        let decoder_thread = thread::Builder::new()
            .name("ghost-backdrop".into())
            .spawn(move || decode(rx, tx))
            .ok();
        let failed = decoder_thread.is_none();
        Self {
            frame: None,
            requests: Some(requests),
            decoder_thread,
            frames,
            waiting: false,
            failed,
        }
    }

    pub fn interval(&self, focused: bool) -> Option<Duration> {
        (focused && !self.failed).then_some(FRAME_INTERVAL)
    }

    pub fn poll(&mut self) {
        match self.frames.try_recv() {
            Ok(Some(frame)) => {
                self.frame = Some(frame);
                self.waiting = false;
            }
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                self.failed = true;
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !self.waiting && !self.failed {
            self.failed = self
                .requests
                .as_ref()
                .is_none_or(|sender| sender.try_send(()).is_err());
            self.waiting = !self.failed;
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(decoder) = self.decoder_thread.take() {
            let _ = decoder.join();
        }
    }
}

#[cfg(not(windows))]
fn decode(_: Receiver<()>, tx: SyncSender<Option<image::Handle>>) {
    let _ = tx.send(None);
}

#[cfg(windows)]
fn decode(requests: Receiver<()>, frames: SyncSender<Option<image::Handle>>) {
    if requests.recv().is_err() {
        return;
    }
    let Ok(file) = write_video() else {
        let _ = frames.send(None);
        return;
    };
    let Some(mut decoder) = native::Decoder::open(file.path()) else {
        let _ = frames.send(None);
        return;
    };
    loop {
        let frame = decoder.read();
        let failed = frame.is_none();
        if frames.send(frame).is_err() || failed {
            break;
        }
        if requests.recv().is_err() {
            break;
        }
    }
    // The file is removed after the native decoder releases its reader.
    drop(decoder);
}

#[cfg(windows)]
fn write_video() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("ghost-citadel-")
        .suffix(".mp4")
        .tempfile()?;
    file.write_all(include_bytes!("../../design/citadel.mp4"))?;
    file.flush()?;
    Ok(file)
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, ptr::NonNull};

    unsafe extern "C" {
        fn ghost_video_open(path: *const u16, width: *mut u32, height: *mut u32) -> *mut c_void;
        fn ghost_video_read(video: *mut c_void, rgba: *mut u8, capacity: usize) -> i32;
        fn ghost_video_close(video: *mut c_void);
    }

    pub struct Decoder {
        pointer: NonNull<c_void>,
        width: u32,
        height: u32,
    }
    impl Decoder {
        pub fn open(path: &Path) -> Option<Self> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let (mut width, mut height) = (0, 0);
            // The native reader and COM apartment remain on this decoder thread.
            let pointer =
                NonNull::new(unsafe { ghost_video_open(path.as_ptr(), &mut width, &mut height) })?;
            Some(Self {
                pointer,
                width,
                height,
            })
        }
        pub fn read(&mut self) -> Option<image::Handle> {
            let mut rgba = vec![0; self.width as usize * self.height as usize * 4];
            // The native reader validates dimensions and writes exactly this buffer.
            let status =
                unsafe { ghost_video_read(self.pointer.as_ptr(), rgba.as_mut_ptr(), rgba.len()) };
            #[cfg(test)]
            assert_eq!(status, 1, "Native decoder status {status}");
            (status == 1).then(|| image::Handle::from_rgba(self.width, self.height, rgba))
        }
    }
    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { ghost_video_close(self.pointer.as_ptr()) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    unsafe extern "C" {
        fn ghost_video_convert_buffer(
            bytes: *const u8,
            length: usize,
            stride: i32,
            width: u32,
            height: u32,
            rgba: *mut u8,
            capacity: usize,
        ) -> bool;
    }
    #[test]
    fn native_plain_buffers_preserve_top_down_and_bottom_up_rows() {
        let source = [3, 2, 1, 0, 0, 0, 0, 0, 6, 5, 4, 0, 0, 0, 0, 0];
        for (stride, expected) in [
            (8, [1, 2, 3, 255, 4, 5, 6, 255]),
            (-8, [4, 5, 6, 255, 1, 2, 3, 255]),
        ] {
            let mut output = [0; 8];
            assert!(unsafe {
                ghost_video_convert_buffer(
                    source.as_ptr(),
                    source.len(),
                    stride,
                    1,
                    2,
                    output.as_mut_ptr(),
                    output.len(),
                )
            });
            assert_eq!(output, expected);
            assert!(!unsafe {
                ghost_video_convert_buffer(
                    source.as_ptr(),
                    4,
                    stride,
                    1,
                    2,
                    output.as_mut_ptr(),
                    output.len(),
                )
            });
        }
    }
    #[test]
    fn native_decoder_crosses_the_loop_boundary() {
        let file = write_video().unwrap();
        let mut decoder = native::Decoder::open(file.path()).expect("Windows Media Foundation");
        let mut first_pixels = Vec::new();
        for index in 0..190 {
            let image::Handle::Rgba { pixels, .. } = decoder.read().expect("Decoded frame") else {
                panic!("Expected native RGBA pixels");
            };
            if index == 0 {
                first_pixels.extend_from_slice(&pixels);
            } else if index == 90 {
                assert_ne!(
                    first_pixels.as_slice(),
                    pixels.as_ref(),
                    "Video must animate"
                );
            }
        }
        drop(decoder);
    }
}

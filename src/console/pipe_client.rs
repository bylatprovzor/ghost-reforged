//! Bounded native connection with impersonation disabled and verified peer identity.
use super::{ipc::Failure, security};
use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode::Bytes};
use std::{
    fs::OpenOptions,
    io,
    os::windows::{fs::OpenOptionsExt, io::OwnedHandle},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::ERROR_PIPE_BUSY,
    Storage::FileSystem::{FILE_FLAG_OVERLAPPED, SECURITY_IDENTIFICATION},
};

pub(super) fn connect(
    name: &str,
    deadline: Instant,
) -> Result<(DuplexPipeStream<Bytes>, OwnedHandle), Failure> {
    let before = security::timestamp();
    let handle = open(name, deadline).map_err(|_| Failure::Transport(
        "No console controller is available (or connection timed out). Start --no-gui for this profile directory.".into()
    ))?;
    let stream = DuplexPipeStream::<Bytes>::try_from(handle)
        .map_err(|_| Failure::Remote("Invalid local controller pipe".into()))?;
    let pid = stream
        .server_process_id()
        .map_err(|_| Failure::Remote("Cannot identify the local controller".into()))?;
    let peer =
        security::authenticate_peer(pid, &security::user_sid().map_err(Failure::Remote)?, before)
            .map_err(Failure::Remote)?;
    if Instant::now() >= deadline {
        return Err(Failure::Remote(
            "Controller authentication timed out; no command was sent".into(),
        ));
    }
    Ok((stream, peer))
}

fn open(name: &str, deadline: Instant) -> io::Result<OwnedHandle> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            // Never let a pipe-name squatter impersonate the CLI's user.
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name)
        {
            Ok(file) => return Ok(file.into()),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::os::windows::named_pipe::PipeListenerOptions;

    #[test]
    fn busy_native_pipe_cannot_hang_connection_past_deadline() {
        let name = format!(
            r"\\.\pipe\ghost-busy-test-{}-{}",
            std::process::id(),
            security::timestamp()
        );
        let _listener = PipeListenerOptions::new()
            .path(name.as_str())
            .instance_limit(std::num::NonZeroU8::new(1))
            .create_duplex::<Bytes>()
            .unwrap();
        let _occupied = open(&name, Instant::now() + Duration::from_secs(1)).unwrap();
        let began = Instant::now();
        let error = open(&name, began + Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(began.elapsed() < Duration::from_secs(1));
    }
}

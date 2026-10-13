//! Same-user named pipe, bounded nonblocking I/O. No network listener or stored token.
use super::{Action, Request, Response, security};
use interprocess::os::windows::named_pipe::{
    DuplexPipeStream, PipeListenerOptions, pipe_mode::Bytes,
};
type Stream = DuplexPipeStream<Bytes>;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const LIMIT: usize = 1024 * 1024;
type Call = (Request, Sender<Response>);
#[derive(Debug)]
pub(super) enum Failure {
    Transport(String),
    Remote(String),
}

fn name(directory: &Path) -> Result<String, String> {
    let canonical = directory
        .canonicalize()
        .map_err(|_| "Profile directory is unavailable")?;
    let hash = Sha256::digest(
        format!(
            "{}\n{}",
            security::user_sid()?,
            canonical.to_string_lossy().to_lowercase()
        )
        .as_bytes(),
    );
    Ok(format!(r"\\.\pipe\ghost-reforged-control-v1-{:x}", hash))
}

pub(super) struct Server {
    calls: Receiver<Call>,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}
impl Server {
    pub fn start(directory: &Path) -> Result<Self, String> {
        let name = name(directory)?;
        let listener = PipeListenerOptions::new()
            .path(name)
            .nonblocking(true)
            .accept_remote(false)
            .input_buffer_size_hint(65536)
            .output_buffer_size_hint(65536)
            .security_descriptor(Some(security::descriptor()?))
            .create_duplex::<Bytes>()
            .map_err(|e| format!("Cannot listen on local control pipe: {e}"))?;
        let (tx, calls) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let serving = busy.clone();
        thread::Builder::new()
            .name("ghost-control-pipe".into())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok(mut stream) => {
                            serving.store(true, Ordering::SeqCst);
                            let deadline = Instant::now() + Duration::from_secs(3);
                            let response = read(&mut stream, deadline, &stopping)
                                .and_then(|bytes| {
                                    let request = decode(&bytes)?;
                                    let (reply, answer) = mpsc::channel();
                                    tx.try_send((request, reply))
                                        .map_err(|_| "Controller is busy")?;
                                    answer.recv_timeout(Duration::from_secs(3)).map_err(|_| {
                                        "Controller response timed out; operation outcome is unknown. Check status before retrying.".to_owned()
                                    })
                                })
                                .unwrap_or_else(|error| Response { version: 1, result: Err(error) });
                            let _ = write(
                                &mut stream,
                                &encode(response),
                                Instant::now() + Duration::from_secs(2),
                                &stopping,
                            );
                            serving.store(false, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| "Cannot start local controller listener")?;
        Ok(Self { calls, stop, busy })
    }
    pub fn receive(&self) -> Result<Option<Call>, String> {
        match self.calls.try_recv() {
            Ok(call) => Ok(Some(call)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Local controller listener stopped unexpectedly".into())
            }
        }
    }
    pub fn idle(&self) -> bool {
        !self.busy.load(Ordering::SeqCst)
    }
}

fn encode(response: Response) -> Vec<u8> {
    match serde_json::to_vec(&response) {
        Ok(bytes) if bytes.len() < LIMIT => bytes,
        _ => serde_json::to_vec(&Response {
            version: 1,
            result: Err("Control response exceeds 1 MiB; operation may have completed. Inspect status before retrying.".into()),
        }).expect("A fixed string response is serializable"),
    }
}

fn decode(bytes: &[u8]) -> Result<Request, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Invalid control request")?;
    let request: Request =
        serde_json::from_value(value.clone()).map_err(|_| "Invalid control request")?;
    // Serde's internally tagged unit variants can ignore fields even with
    // deny_unknown_fields. Check the canonical action's keys as well.
    let canonical = serde_json::to_value(&request.action).map_err(|_| "Invalid control request")?;
    let fields = value["action"]
        .as_object()
        .ok_or("Invalid control request")?;
    let allowed = canonical.as_object().ok_or("Invalid control request")?;
    if fields.keys().any(|key| !allowed.contains_key(key)) {
        return Err("Invalid control request: unknown action field".into());
    }
    Ok(request)
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub(super) fn request(directory: &Path, action: Action) -> Result<serde_json::Value, Failure> {
    let name = name(directory).map_err(Failure::Transport)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let (mut stream, _peer) = super::pipe_client::connect(&name, deadline)?;
    stream
        .set_nonblocking(true)
        .map_err(|e| Failure::Remote(e.to_string()))?;
    let bytes = serde_json::to_vec(&Request { version: 1, action })
        .map_err(|e| Failure::Remote(e.to_string()))?;
    let stop = AtomicBool::new(false);
    // After connection, never fall back to offline mutation on an uncertain response.
    write(&mut stream, &bytes, deadline, &stop).map_err(Failure::Remote)?;
    let bytes = read(&mut stream, deadline, &stop).map_err(Failure::Remote)?;
    let response: Response = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::Remote("Invalid controller response".into()))?;
    if response.version != 1 {
        return Err(Failure::Remote("Unsupported controller version".into()));
    }
    response.result.map_err(Failure::Remote)
}

fn check(deadline: Instant, stop: &AtomicBool) -> Result<(), String> {
    if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
        Err(
            "Control I/O timed out; command outcome is unknown. Inspect status before retrying."
                .into(),
        )
    } else {
        Ok(())
    }
}
fn read(stream: &mut Stream, deadline: Instant, stop: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        check(deadline, stop)?;
        match stream.read(&mut buffer) {
            // PIPE_NOWAIT can produce an empty read before a peer's reply. Only a
            // complete framed message is success; idle/disconnected peers time out.
            Ok(0) => thread::sleep(Duration::from_millis(10)),
            Ok(count) => {
                if result.len() + count > LIMIT {
                    return Err("Control message exceeds 1 MiB".into());
                }
                result.extend_from_slice(&buffer[..count]);
                if let Some(end) = result.iter().position(|b| *b == b'\n') {
                    if end + 1 != result.len() {
                        return Err("Only one control request per connection is allowed".into());
                    }
                    result.truncate(end);
                    return Ok(result);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(_) => {
                return Err(
                    "Controller connection failed; operation outcome may be unknown".into(),
                );
            }
        }
    }
}
fn write(
    stream: &mut Stream,
    bytes: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<(), String> {
    if bytes.len() + 1 > LIMIT {
        return Err("Control message exceeds 1 MiB".into());
    }
    let mut bytes = bytes.to_vec();
    bytes.push(b'\n');
    let mut sent = 0;
    while sent < bytes.len() {
        check(deadline, stop)?;
        // Keep writes below the configured native buffer even for large profile/map lists.
        match stream.write(&bytes[sent..(sent + 4096).min(bytes.len())]) {
            Ok(0) => thread::sleep(Duration::from_millis(10)),
            Ok(count) => sent += count,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(_) => {
                return Err(
                    "Controller connection failed; operation outcome may be unknown".into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_reply_is_an_explicit_error_not_a_silent_timeout() {
        let response: Response = serde_json::from_slice(&encode(Response {
            version: 1,
            result: Ok(serde_json::json!({"large": "a".repeat(LIMIT)})),
        }))
        .unwrap();
        assert!(response.result.unwrap_err().contains("exceeds 1 MiB"));
    }

    #[test]
    fn native_pipe_rejects_unknown_fields_without_dispatching_and_accepts_next_connection() {
        let directory = tempfile::tempdir().unwrap();
        let server = Server::start(directory.path()).unwrap();
        for bytes in [
            br#"{"version":1,"action":{"command":"quit","unexpected":true}}"#.as_slice(),
            br#"{"version":1,"action":{"command":"status"},"unexpected":true}"#.as_slice(),
            b"not json".as_slice(),
        ] {
            let mut stream = Stream::connect_by_path(name(directory.path()).unwrap()).unwrap();
            stream.set_nonblocking(true).unwrap();
            let stop = AtomicBool::new(false);
            let deadline = Instant::now() + Duration::from_secs(3);
            write(&mut stream, bytes, deadline, &stop).unwrap();
            let response: Response =
                serde_json::from_slice(&read(&mut stream, deadline, &stop).unwrap()).unwrap();
            assert!(response.result.is_err());
            assert!(server.receive().unwrap().is_none());
        }
    }

    #[test]
    fn dead_listener_is_not_reported_as_an_idle_healthy_controller() {
        let (sender, calls) = mpsc::channel();
        drop(sender);
        let server = Server {
            calls,
            stop: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
        };
        assert!(server.receive().is_err());
    }
}

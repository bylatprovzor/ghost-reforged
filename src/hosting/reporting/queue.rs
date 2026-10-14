use super::StartedGame;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

pub(super) fn random_id() -> io::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}

pub(super) fn identity(directory: &Path) -> io::Result<String> {
    fs::create_dir_all(directory)?;
    let path = directory.join("bot-id");
    if !path.exists() {
        atomic(&path, format!("ghost-{}", random_id()?).as_bytes())?;
    }
    let id = fs::read_to_string(path)?;
    if id.len() != 38
        || !id.starts_with("ghost-")
        || !id[6..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::other("Invalid saved bot identity"));
    }
    Ok(id)
}

pub(super) fn save(directory: &Path, game: &StartedGame) -> io::Result<()> {
    if game.game_id.len() != 32 || !game.game_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other("Invalid event ID"));
    }
    let bytes = serde_json::to_vec(game)?;
    let path = directory.join(format!("{}.json", game.game_id));
    if path.exists() {
        if fs::read(path)? == bytes {
            return Ok(());
        }
        return Err(io::Error::other("Conflicting saved start"));
    }
    atomic(&path, &bytes)
}

#[derive(Debug, PartialEq)]
enum Delivery {
    Sent,
    Retry,
    Rejected,
}

fn outcome(status: u16) -> Delivery {
    match status {
        200 | 201 => Delivery::Sent,
        408 | 429 | 500..=599 | 401 | 403 => Delivery::Retry,
        _ => Delivery::Rejected,
    }
}

fn deliver(agent: &ureq::Agent, endpoint: &str, key: &str, body: &str) -> Delivery {
    match agent
        .post(endpoint)
        .header("Authorization", &format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send(body)
    {
        Ok(mut response) => {
            if outcome(response.status().as_u16()) != Delivery::Sent {
                return outcome(response.status().as_u16());
            }
            let ack = response
                .body_mut()
                .with_config()
                .limit(16384)
                .read_to_string()
                .ok()
                .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok());
            let expected = serde_json::from_str::<StartedGame>(body).ok();
            if ack.zip(expected).is_some_and(|(ack, game)| {
                ack["game_id"] == game.game_id && ack["duplicate"].is_boolean()
            }) {
                Delivery::Sent
            } else {
                Delivery::Retry
            }
        }
        Err(ureq::Error::StatusCode(status)) => outcome(status),
        Err(_) => Delivery::Retry,
    }
}

fn drain(directory: &Path, mut send: impl FnMut(&str) -> Delivery) -> io::Result<(usize, bool)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let mut sent = 0;
    let mut retry = false;
    for path in paths.into_iter().take(100) {
        let bytes = fs::read(&path)?;
        let result = match serde_json::from_slice::<StartedGame>(&bytes) {
            Ok(_) => send(std::str::from_utf8(&bytes).map_err(io::Error::other)?),
            Err(_) => Delivery::Rejected,
        };
        match result {
            Delivery::Sent => {
                fs::remove_file(path)?;
                sent += 1;
            }
            Delivery::Rejected => {
                fs::rename(&path, path.with_extension("rejected"))?;
            }
            Delivery::Retry => {
                retry = true;
                break;
            }
        }
    }
    Ok((sent, retry))
}

pub(super) fn run(
    directory: PathBuf,
    key: String,
    rx: mpsc::Receiver<()>,
    tx: mpsc::Sender<String>,
) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut delay = 2u64;
    loop {
        let mut rejected = 0;
        match drain(&directory, |body| {
            let result = deliver(&agent, "https://api.w3ladder.net/v1/bot/games", &key, body);
            if result == Delivery::Rejected {
                rejected += 1;
            }
            result
        }) {
            Ok((count, retry)) => {
                if count > 0 {
                    let _ = tx.send(format!(
                        "{count} started game(s) registered on w3ladder.net."
                    ));
                }
                if retry {
                    delay = (delay * 2).min(120);
                } else {
                    delay = 2;
                }
            }
            Err(_) => {
                let _ = tx
                    .send("Game report queue could not be read; saved events are retained.".into());
                delay = 120;
            }
        }
        if rejected > 0 {
            let _ = tx.send(format!(
                "{rejected} game report(s) rejected; original events retained for inspection."
            ));
        }
        let mut jitter = [0u8; 1];
        let _ = getrandom::fill(&mut jitter);
        match rx.recv_timeout(Duration::from_secs(delay + u64::from(jitter[0] % 4))) {
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosting::reporting::tests::game;

    #[test]
    fn native_http_ack_must_confirm_the_same_game_before_removing_it() {
        use std::{io::Read, net::TcpListener};
        for (ack, expected) in [
            (
                serde_json::json!({"game_id":game().game_id,"duplicate":false}),
                Delivery::Sent,
            ),
            (
                serde_json::json!({"game_id":"different-game","duplicate":true}),
                Delivery::Retry,
            ),
            (serde_json::json!({"status":"ok"}), Delivery::Retry),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 4096];
                loop {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = ack.to_string();
                write!(
                    socket,
                    "HTTP/1.1 201 Created\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            });
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(3)))
                .build()
                .new_agent();
            assert_eq!(
                deliver(
                    &agent,
                    &format!("http://{address}/v1/bot/games"),
                    "fixture-key",
                    &serde_json::to_string(&game()).unwrap()
                ),
                expected
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn retries_reopen_the_same_durable_event_and_quarantine_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let id = identity(temp.path()).unwrap();
        assert_eq!(identity(temp.path()).unwrap(), id);
        let event = game();
        save(temp.path(), &event).unwrap();
        let mut first = String::new();
        assert_eq!(
            drain(temp.path(), |body| {
                first = body.into();
                Delivery::Retry
            })
            .unwrap(),
            (0, true)
        );
        assert_eq!(
            drain(temp.path(), |body| {
                assert_eq!(body, first);
                Delivery::Sent
            })
            .unwrap(),
            (1, false)
        );
        assert!(!temp.path().join(format!("{}.json", event.game_id)).exists());
        save(temp.path(), &event).unwrap();
        assert_eq!(
            drain(temp.path(), |_| Delivery::Rejected).unwrap(),
            (0, false)
        );
        assert!(
            temp.path()
                .join(format!("{}.rejected", event.game_id))
                .exists()
        );
    }

    #[test]
    fn duplicates_are_immutable_and_paths_cannot_escape_the_queue() {
        let temp = tempfile::tempdir().unwrap();
        let mut event = game();
        save(temp.path(), &event).unwrap();
        save(temp.path(), &event).unwrap();
        event.game_name = "Different".into();
        assert!(save(temp.path(), &event).is_err());
        event.game_id = "../escape".into();
        assert!(save(temp.path(), &event).is_err());
        for code in [408, 429, 500, 503, 401, 403] {
            assert_eq!(outcome(code), Delivery::Retry);
        }
        for code in [400, 409, 422, 302] {
            assert_eq!(outcome(code), Delivery::Rejected);
        }
    }
}

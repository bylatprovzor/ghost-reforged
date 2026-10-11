use super::*;
use crate::discovery::Endpoint;
use std::{net::TcpListener, thread};
use tungstenite::{Message, WebSocket};

fn event(kind: &str, payload: Value) -> Value {
    json!({"messageType":kind,"payload":payload})
}

fn lobby() -> Value {
    json!({"isHost":true,"lobbyName":"Alias test", "mapData":{"mapPath":"C:/Maps/test.w3x"},
        "players":[{"isSelf":true,"name":"Ghost"}],"playerHost":"Ghost"})
}

fn connection(
    script: impl FnOnce(WebSocket<std::net::TcpStream>) + Send + 'static,
) -> (Client, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        script(tungstenite::accept(stream).unwrap());
    });
    let client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "1234567890123".into(),
        version: None,
    })
    .unwrap();
    (client, server)
}

#[test]
fn a_loading_event_during_alias_verification_is_preserved_without_another_query() {
    let loading = event("SetGlueScreen", json!({"screen":"LOADING_SCREEN"}));
    let emitted = loading.clone();
    let (mut client, server) = connection(move |mut socket| {
        socket
            .send(Message::Text(emitted.to_string().into()))
            .unwrap();
        assert!(
            socket.read().is_err(),
            "No lobby request should follow loading"
        );
    });
    let mut pending = Vec::new();
    let observed =
        fresh_lobby(&mut client, &lobby(), &AtomicBool::new(false), &mut pending).unwrap();
    assert!(observed.is_none());
    assert_eq!(pending, vec![loading]);
    drop(client);
    server.join().unwrap();
}

#[test]
fn countdown_events_are_replayed_and_a_fresh_lobby_is_returned() {
    let countdown = event(
        "LobbyCountdown",
        json!({"details":{"countdownActive":true,"countdownLocked":false}}),
    );
    let emitted = countdown.clone();
    let (mut client, server) = connection(move |mut socket| {
        socket
            .send(Message::Text(emitted.to_string().into()))
            .unwrap();
        let request = socket.read().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&request).unwrap()["message"],
            "SendGameLobbySetup"
        );
        socket
            .send(Message::Text(
                event("GameLobbySetup", lobby()).to_string().into(),
            ))
            .unwrap();
    });
    let mut pending = Vec::new();
    let observed =
        fresh_lobby(&mut client, &lobby(), &AtomicBool::new(false), &mut pending).unwrap();
    assert_eq!(observed, Some(lobby()));
    assert_eq!(pending, vec![countdown]);
    server.join().unwrap();
}

#[test]
fn leaving_and_logging_out_interrupt_name_writes() {
    for kind in [
        "LoggedOut",
        "MultiplayerGameLeave",
        "GameClosed",
        "ExitedGame",
    ] {
        let mut pending = Vec::new();
        let value = event(kind, json!({}));
        assert!(defer_event(value.clone(), &mut pending));
        assert_eq!(pending, vec![value]);
    }
}

#[test]
fn roster_replacement_invalidates_discovery_even_when_population_is_unchanged() {
    let mut before = lobby();
    before["players"][0]["id"] = json!(10);
    let mut after = before.clone();
    after["players"][0]["name"] = json!("Ghost");
    assert!(same_roster(&before, &after));
    for field in ["id", "slot", "slotStatus", "slotType", "team"] {
        let mut after = before.clone();
        after["players"][0][field] = json!(99);
        assert!(!same_roster(&before, &after), "{field}");
    }
    after["players"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":11,"slot":1}));
    assert!(!same_roster(&before, &after));
}

#[test]
fn stopping_before_or_during_alias_confirmation_is_not_a_failure() {
    for before_request in [false, true] {
        let cancelled = std::sync::Arc::new(AtomicBool::new(before_request));
        let signal = cancelled.clone();
        let (mut client, server) = connection(move |mut socket| {
            if !before_request {
                let request: Value =
                    serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
                assert_eq!(request["message"], "SendGameLobbySetup");
                signal.store(true, Ordering::SeqCst);
                socket
                    .send(Message::Text(event("Other", json!({})).to_string().into()))
                    .unwrap();
            }
            assert!(
                socket.read().is_err(),
                "Stopping must not send another query"
            );
        });
        let mut pending = Vec::new();
        let outcome = fresh_lobby(&mut client, &lobby(), &cancelled, &mut pending);
        drop(client);
        server.join().unwrap();
        assert!(outcome.unwrap().is_none());
    }
}

#[cfg(windows)]
#[test]
fn overwritten_host_names_are_restored_on_every_lobby_update() {
    #[repr(C, align(8))]
    struct Inline(std::cell::UnsafeCell<[u8; 40]>);
    let object = Box::new(Inline(std::cell::UnsafeCell::new([0; 40])));
    let address = object.0.get() as usize;
    let mut original = [0u8; 40];
    original[..8].copy_from_slice(&((address + 24) as u64).to_le_bytes());
    original[8..16].copy_from_slice(&5u64.to_le_bytes());
    original[16..24].copy_from_slice(&0x800000000000000fu64.to_le_bytes());
    original[24..29].copy_from_slice(b"Owner");
    let (mut client, server) = connection(move |mut socket| {
        for _ in 0..6 {
            let request: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(request["message"], "SendGameLobbySetup");
            // Each reply serializes the current native object, as the game does.
            let bytes = unsafe { *(address as *const [u8; 40]) };
            let name = std::str::from_utf8(&bytes[24..29]).unwrap();
            let mut state = lobby();
            state["playerHost"] = json!(name);
            state["players"][0]["name"] = json!(name);
            socket
                .send(Message::Text(
                    event("GameLobbySetup", state).to_string().into(),
                ))
                .unwrap();
        }
    });
    let mut alias = Alias::open(std::process::id(), "Ghost".into()).unwrap();
    alias.candidates.verified = Some(address);
    for _ in 0..3 {
        unsafe {
            *object.0.get() = original;
        }
        let mut updated = lobby();
        updated["playerHost"] = json!("Owner");
        updated["players"][0]["name"] = json!("Owner");
        assert!(!alias.matches(&updated));
        let Verification::Verified { snapshot, pending } = alias
            .ensure(&mut client, &updated, &AtomicBool::new(false))
            .unwrap()
        else {
            panic!("The updated host name was not restored");
        };
        assert!(alias.matches(&snapshot));
        assert!(pending.is_empty());
        let applied = unsafe { *object.0.get() };
        assert_eq!(&applied[24..29], b"Ghost");
    }
    alias.reset();
    assert_eq!(unsafe { *object.0.get() }, original);
    assert!(alias.candidates.verified.is_none());
    server.join().unwrap();
}

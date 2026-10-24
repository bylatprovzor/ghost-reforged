use super::*;
use std::{net::TcpListener, sync::mpsc, thread};

#[test]
fn handshake_failures_distinguish_refusal_from_timeout_without_exposing_the_session() {
    use std::io::{Read, Write};
    for refuse in [true, false] {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).unwrap();
            if refuse {
                stream
                    .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();
            } else {
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        let error = match Client::connect(&Endpoint {
            pid: 0,
            ports: vec![port],
            session: "private-session-value".into(),
            version: None,
        }) {
            Ok(_) => panic!("Unresponsive or rejecting listener was accepted"),
            Err(error) => error,
        };
        if refuse {
            assert!(matches!(error, TransportError::HandshakeRejected(403)));
        } else {
            assert!(matches!(error, TransportError::HandshakeTimeout));
            done.send(()).unwrap();
        }
        assert!(!error.to_string().contains("private-session-value"));
        server.join().unwrap();
    }
}

#[test]
fn keepalives_do_not_mark_a_queued_roster_as_idle() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        socket.send(Message::Ping(vec![1].into())).unwrap();
        socket.send(Message::Pong(vec![2].into())).unwrap();
        socket
            .send(Message::Text(
                json!({"messageType":"GameLobbySetup"}).to_string().into(),
            ))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let mut client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "1234567890123".into(),
        version: None,
    })
    .unwrap();
    assert_eq!(client.receive().unwrap(), Some(Value::Null));
    assert_eq!(client.receive().unwrap(), Some(Value::Null));
    assert_eq!(
        client.receive().unwrap().unwrap()["messageType"],
        "GameLobbySetup"
    );
    assert!(client.receive_available().unwrap().is_none());
    tx.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn countdown_polling_uses_a_short_native_socket_deadline_and_preserves_it_after_draining() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let _socket = tungstenite::accept(stream).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let mut client = Client::connect(&Endpoint {
        pid: 0,
        ports: vec![port],
        session: "1234567890123".into(),
        version: None,
    })
    .unwrap();
    let short = Duration::from_millis(50);
    let started = std::time::Instant::now();
    assert!(client.receive_with_timeout(short).unwrap().is_none());
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "The idle 250ms wait must not delay countdown refreshes"
    );
    assert_eq!(client.0.get_ref().read_timeout().unwrap(), Some(short));
    assert!(client.receive_available().unwrap().is_none());
    assert_eq!(client.0.get_ref().read_timeout().unwrap(), Some(short));
    let idle = Duration::from_millis(250);
    assert!(client.receive_with_timeout(idle).unwrap().is_none());
    assert_eq!(client.0.get_ref().read_timeout().unwrap(), Some(idle));
    tx.send(()).unwrap();
    server.join().unwrap();
}

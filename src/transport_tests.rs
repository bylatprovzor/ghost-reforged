use super::*;
use std::{net::TcpListener, sync::mpsc, thread};

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

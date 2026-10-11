use crate::discovery::Endpoint;
use serde_json::{Value, json};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("Warcraft III rejected the local UI connection")]
    Connect,
    #[error("Local UI connection closed or failed")]
    Disconnected,
    #[error("Invalid JSON from the local UI")]
    Json(#[from] serde_json::Error),
}

pub struct Client(WebSocket<TcpStream>);
impl Client {
    pub fn connect(endpoint: &Endpoint) -> Result<Self, TransportError> {
        for &port in &endpoint.ports {
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(2)) else {
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let url = format!("ws://127.0.0.1:{port}/webui-socket/{}", endpoint.session);
            let config = WebSocketConfig::default()
                .max_message_size(Some(8 * 1024 * 1024))
                .max_frame_size(Some(8 * 1024 * 1024));
            if let Ok((socket, _)) =
                tungstenite::client::client_with_config(url, stream, Some(config))
            {
                let _ = socket
                    .get_ref()
                    .set_read_timeout(Some(Duration::from_millis(250)));
                return Ok(Self(socket));
            }
        }
        Err(TransportError::Connect)
    }

    pub fn send(&mut self, command: &str, payload: Value) -> Result<(), TransportError> {
        self.0
            .send(Message::Text(
                json!({"type": "webui", "message": command, "payload": payload})
                    .to_string()
                    .into(),
            ))
            .map_err(|_| TransportError::Disconnected)
    }

    pub fn receive(&mut self) -> Result<Option<Value>, TransportError> {
        match self.0.read() {
            Ok(Message::Text(text)) => Ok(Some(serde_json::from_str(&text)?)),
            Ok(Message::Ping(_)) => {
                self.0.flush().map_err(|_| TransportError::Disconnected)?;
                Ok(None)
            }
            Ok(Message::Close(_)) => Err(TransportError::Disconnected),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            Err(_) => Err(TransportError::Disconnected),
        }
    }

    /// Drain queued callbacks without paying the normal idle polling delay.
    pub fn receive_available(&mut self) -> Result<Option<Value>, TransportError> {
        self.0
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(5)))
            .map_err(|_| TransportError::Disconnected)?;
        let received = self.receive();
        self.0
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|_| TransportError::Disconnected)?;
        received
    }
}

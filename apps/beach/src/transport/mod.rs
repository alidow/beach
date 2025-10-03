use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub mod ipc;
pub mod webrtc;
pub mod websocket;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransportId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportKind {
    WebRtc,
    WebSocket,
    Ipc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Binary(Vec<u8>),
    Text(String),
}

impl Payload {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Payload::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Payload::Binary(bytes) => bytes,
            Payload::Text(text) => text.into_bytes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportMessage {
    pub sequence: u64,
    pub payload: Payload,
}

impl TransportMessage {
    pub fn binary(sequence: u64, bytes: Vec<u8>) -> Self {
        Self {
            sequence,
            payload: Payload::Binary(bytes),
        }
    }

    pub fn text(sequence: u64, text: impl Into<String>) -> Self {
        Self {
            sequence,
            payload: Payload::Text(text.into()),
        }
    }
}

const MESSAGE_HEADER_LEN: usize = 1 + 8 + 4;

pub(crate) fn encode_message(message: &TransportMessage) -> Vec<u8> {
    let (payload_type, data) = match &message.payload {
        Payload::Text(text) => (0u8, text.as_bytes().to_vec()),
        Payload::Binary(bytes) => (1u8, bytes.clone()),
    };
    let mut buf = Vec::with_capacity(MESSAGE_HEADER_LEN + data.len());
    buf.push(payload_type);
    buf.extend_from_slice(&message.sequence.to_be_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(&data);
    buf
}

pub(crate) fn decode_message(bytes: &[u8]) -> Option<TransportMessage> {
    if bytes.len() < MESSAGE_HEADER_LEN {
        return None;
    }
    let payload_type = bytes[0];
    let sequence = u64::from_be_bytes(bytes[1..9].try_into().ok()?);
    let len = u32::from_be_bytes(bytes[9..13].try_into().ok()?) as usize;
    if bytes.len() < MESSAGE_HEADER_LEN + len {
        return None;
    }
    let data = &bytes[13..13 + len];
    let payload = match payload_type {
        0 => Payload::Text(String::from_utf8(data.to_vec()).ok()?),
        1 => Payload::Binary(data.to_vec()),
        _ => return None,
    };
    Some(TransportMessage { sequence, payload })
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("transport channel closed")]
    ChannelClosed,
    #[error("transport receive timeout")]
    Timeout,
    #[error("transport setup failed: {0}")]
    Setup(String),
}

pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;
    fn id(&self) -> TransportId;
    fn peer(&self) -> TransportId;
    fn send(&self, message: TransportMessage) -> Result<(), TransportError>;
    fn send_text(&self, text: &str) -> Result<u64, TransportError>;
    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError>;
    fn recv(&self, timeout: Duration) -> Result<TransportMessage, TransportError>;
    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError>;
}

pub(crate) fn next_transport_id() -> TransportId {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    TransportId(COUNTER.fetch_add(1, Ordering::Relaxed))
}

struct EndpointCore {
    sender: mpsc::Sender<TransportMessage>,
    receiver: Mutex<mpsc::Receiver<TransportMessage>>,
    outbound_seq: AtomicU64,
}

impl EndpointCore {
    fn new(
        sender: mpsc::Sender<TransportMessage>,
        receiver: mpsc::Receiver<TransportMessage>,
    ) -> Self {
        Self {
            sender,
            receiver: Mutex::new(receiver),
            outbound_seq: AtomicU64::new(0),
        }
    }
}

pub struct TransportEndpoint {
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    core: Arc<EndpointCore>,
}

impl TransportEndpoint {
    fn new(
        kind: TransportKind,
        id: TransportId,
        peer: TransportId,
        core: Arc<EndpointCore>,
    ) -> Self {
        Self {
            kind,
            id,
            peer,
            core,
        }
    }
}

impl Transport for TransportEndpoint {
    fn kind(&self) -> TransportKind {
        self.kind
    }
    fn id(&self) -> TransportId {
        self.id
    }
    fn peer(&self) -> TransportId {
        self.peer
    }

    fn send(&self, message: TransportMessage) -> Result<(), TransportError> {
        self.core
            .sender
            .send(message)
            .map_err(|_| TransportError::ChannelClosed)
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        let sequence = self.core.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::text(sequence, text.to_string()))?;
        Ok(sequence)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        let sequence = self.core.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::binary(sequence, bytes.to_vec()))?;
        Ok(sequence)
    }

    fn recv(&self, timeout: Duration) -> Result<TransportMessage, TransportError> {
        let receiver = self.core.receiver.lock().unwrap();
        match receiver.recv_timeout(timeout) {
            Ok(message) => Ok(message),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::ChannelClosed),
        }
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        let receiver = self.core.receiver.lock().unwrap();
        match receiver.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(TransportError::ChannelClosed),
        }
    }
}

pub struct TransportPair {
    pub client: Box<dyn Transport>,
    pub server: Box<dyn Transport>,
}

impl TransportPair {
    pub fn new(kind: TransportKind) -> Self {
        let (client_to_server_tx, client_to_server_rx) = mpsc::channel();
        let (server_to_client_tx, server_to_client_rx) = mpsc::channel();

        let client_core = Arc::new(EndpointCore::new(client_to_server_tx, server_to_client_rx));
        let server_core = Arc::new(EndpointCore::new(server_to_client_tx, client_to_server_rx));

        let client_id = next_transport_id();
        let server_id = next_transport_id();

        let client = TransportEndpoint::new(kind, client_id, server_id, client_core);
        let server = TransportEndpoint::new(kind, server_id, client_id, server_core);

        Self {
            client: Box::new(client),
            server: Box::new(server),
        }
    }
}

pub trait TransportBuilder {
    fn build_pair(&self) -> Result<TransportPair, TransportError>;
}

pub struct WebRtcBuilder;

impl TransportBuilder for WebRtcBuilder {
    fn build_pair(&self) -> Result<TransportPair, TransportError> {
        webrtc::build_pair()
    }
}

pub struct WebSocketBuilder;

impl TransportBuilder for WebSocketBuilder {
    fn build_pair(&self) -> Result<TransportPair, TransportError> {
        websocket::build_pair()
    }
}

pub struct IpcBuilder;

impl TransportBuilder for IpcBuilder {
    fn build_pair(&self) -> Result<TransportPair, TransportError> {
        ipc::build_pair()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(builder: &dyn TransportBuilder) {
        let pair = builder.build_pair().expect("pair");
        let timeout = Duration::from_millis(50);

        let client = &pair.client;
        let server = &pair.server;

        let seq_client = client.send_text("hello from client").expect("send");
        let seq_server = server.send_text("hello from server").expect("send");

        let server_msg = server.recv(timeout).expect("recv server");
        assert_eq!(server_msg.sequence, seq_client);
        assert_eq!(server_msg.payload.as_text(), Some("hello from client"));

        let client_msg = client.recv(timeout).expect("recv client");
        assert_eq!(client_msg.sequence, seq_server);
        assert_eq!(client_msg.payload.as_text(), Some("hello from server"));

        assert_eq!(client.kind(), server.kind());
        assert_eq!(client.peer(), server.id());
        assert_eq!(server.peer(), client.id());
    }

    #[test_timeout::timeout]
    fn webrtc_transport_round_trip() {
        round_trip(&WebRtcBuilder);
    }

    #[test_timeout::timeout]
    fn websocket_transport_round_trip() {
        round_trip(&WebSocketBuilder);
    }

    #[test_timeout::timeout]
    fn ipc_transport_round_trip() {
        round_trip(&IpcBuilder);
    }
}

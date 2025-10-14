use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use tokio::io::duplex;
use tokio::runtime::Runtime;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::{
    WebSocketStream, connect_async, tungstenite::Message, tungstenite::protocol::Role,
};

use crate::transport::{
    Payload, Transport, TransportError, TransportId, TransportKind, TransportMessage,
    TransportPair, decode_message, encode_message, next_transport_id,
};

static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("tokio runtime"));

pub fn build_pair() -> Result<TransportPair, TransportError> {
    RUNTIME.block_on(async { create_websocket_pair().await })
}

pub async fn connect(url: &str) -> Result<Box<dyn Transport>, TransportError> {
    let (stream, _resp) = connect_async(url)
        .await
        .map_err(|err| TransportError::Setup(format!("websocket connect failed: {err}")))?;

    let id = next_transport_id();
    let peer = TransportId(0);

    Ok(Box::new(WebSocketTransport::new(
        TransportKind::WebSocket,
        id,
        peer,
        stream,
    )))
}

pub fn wrap_stream<S>(
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    stream: WebSocketStream<S>,
) -> Box<dyn Transport>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    Box::new(WebSocketTransport::new(kind, id, peer, stream))
}

struct WebSocketTransport {
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    outbound_seq: AtomicU64,
    outbound_tx: tokio_mpsc::UnboundedSender<OutboundFrame>,
    inbound_rx: Mutex<mpsc::Receiver<TransportMessage>>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

enum OutboundFrame {
    Text(String),
    Binary(Vec<u8>),
}

impl WebSocketTransport {
    fn new<S>(
        kind: TransportKind,
        id: TransportId,
        peer: TransportId,
        stream: WebSocketStream<S>,
    ) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, mut outbound_rx) = tokio_mpsc::unbounded_channel::<OutboundFrame>();

        let (mut write_half, mut read_half) = stream.split();

        let inbound_sender = inbound_tx.clone();
        let read_task = RUNTIME.spawn(async move {
            while let Some(msg) = read_half.next().await {
                match msg {
                    Ok(Message::Binary(bytes)) => {
                        if let Some(message) = decode_message(&bytes) {
                            let _ = inbound_sender.send(message);
                        }
                    }
                    Ok(Message::Text(text)) => {
                        let payload = Payload::Text(text);
                        let _ = inbound_sender.send(TransportMessage {
                            sequence: 0,
                            payload,
                        });
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        tracing::debug!(
                            target = "transport::websocket",
                            error = %err,
                            "websocket recv error"
                        );
                        break;
                    }
                }
            }
        });

        let write_task = RUNTIME.spawn(async move {
            while let Some(frame) = outbound_rx.recv().await {
                let result = match frame {
                    OutboundFrame::Text(text) => write_half.send(Message::Text(text)).await,
                    OutboundFrame::Binary(bytes) => write_half.send(Message::Binary(bytes)).await,
                };
                if let Err(err) = result {
                    tracing::debug!(
                        target = "transport::websocket",
                        error = %err,
                        "websocket send error"
                    );
                    break;
                }
            }
        });

        Self {
            kind,
            id,
            peer,
            outbound_seq: AtomicU64::new(0),
            outbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            _tasks: vec![read_task, write_task],
        }
    }
}

impl Transport for WebSocketTransport {
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
        match &message.payload {
            Payload::Text(text) => self
                .outbound_tx
                .send(OutboundFrame::Text(text.clone()))
                .map_err(|_| TransportError::ChannelClosed),
            Payload::Binary(_) => {
                let bytes = encode_message(&message);
                self.outbound_tx
                    .send(OutboundFrame::Binary(bytes))
                    .map_err(|_| TransportError::ChannelClosed)
            }
        }
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        let sequence = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::text(sequence, text.to_string()))?;
        Ok(sequence)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        let sequence = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::binary(sequence, bytes.to_vec()))?;
        Ok(sequence)
    }

    fn recv(&self, timeout_duration: Duration) -> Result<TransportMessage, TransportError> {
        let receiver = self.inbound_rx.lock().unwrap();
        receiver
            .recv_timeout(timeout_duration)
            .map_err(|err| match err {
                mpsc::RecvTimeoutError::Timeout => TransportError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => TransportError::ChannelClosed,
            })
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        let receiver = self.inbound_rx.lock().unwrap();
        match receiver.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(TransportError::ChannelClosed),
        }
    }
}

async fn create_websocket_pair() -> Result<TransportPair, TransportError> {
    let (client_raw, server_raw) = duplex(64 * 1024);

    let client_stream = WebSocketStream::from_raw_socket(client_raw, Role::Client, None).await;
    let server_stream = WebSocketStream::from_raw_socket(server_raw, Role::Server, None).await;

    let client_id = next_transport_id();
    let server_id = next_transport_id();

    let client_transport = WebSocketTransport::new(
        TransportKind::WebSocket,
        client_id,
        server_id,
        client_stream,
    );
    let server_transport = WebSocketTransport::new(
        TransportKind::WebSocket,
        server_id,
        client_id,
        server_stream,
    );

    Ok(TransportPair {
        client: Box::new(client_transport),
        server: Box::new(server_transport),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn websocket_pair_round_trip() {
        let pair = build_pair().expect("create websocket pair");
        let timeout = Duration::from_secs(2);

        let client = &pair.client;
        let server = &pair.server;

        assert_eq!(client.kind(), TransportKind::WebSocket);
        assert_eq!(server.kind(), TransportKind::WebSocket);

        client.send_text("ping from client").expect("client send");
        server.send_text("pong from server").expect("server send");

        let server_msg = server.recv(timeout).expect("server recv");
        assert_eq!(server_msg.payload.as_text(), Some("ping from client"));

        let client_msg = client.recv(timeout).expect("client recv");
        assert_eq!(client_msg.payload.as_text(), Some("pong from server"));
    }
}

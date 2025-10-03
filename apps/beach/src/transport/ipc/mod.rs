use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use once_cell::sync::Lazy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;
use tokio::sync::mpsc as tokio_mpsc;

#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

use crate::transport::{
    Transport, TransportError, TransportId, TransportKind, TransportMessage, TransportPair,
    decode_message, encode_message, next_transport_id,
};

static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("tokio runtime"));

pub fn build_pair() -> Result<TransportPair, TransportError> {
    RUNTIME.block_on(async { create_ipc_pair().await })
}

struct IpcTransport {
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    outbound_seq: AtomicU64,
    outbound_tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
    inbound_rx: Mutex<mpsc::Receiver<TransportMessage>>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl IpcTransport {
    fn new<S>(kind: TransportKind, id: TransportId, peer: TransportId, stream: S) -> Self
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, mut outbound_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (mut reader, mut writer) = tokio::io::split(stream);

        let inbound_sender = inbound_tx.clone();
        let read_task = RUNTIME.spawn(async move {
            loop {
                let mut len_buf = [0u8; 4];
                if reader.read_exact(&mut len_buf).await.is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                if reader.read_exact(&mut buf).await.is_err() {
                    break;
                }
                if let Some(message) = decode_message(&buf) {
                    if inbound_sender.send(message).is_err() {
                        break;
                    }
                }
            }
        });

        let write_task = RUNTIME.spawn(async move {
            while let Some(bytes) = outbound_rx.recv().await {
                let len = bytes.len() as u32;
                if writer.write_all(&len.to_be_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
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

impl Transport for IpcTransport {
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
        let bytes = encode_message(&message);
        self.outbound_tx
            .send(bytes)
            .map_err(|_| TransportError::ChannelClosed)
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        let seq = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::text(seq, text.to_string()))?;
        Ok(seq)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        let seq = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::binary(seq, bytes.to_vec()))?;
        Ok(seq)
    }

    fn recv(&self, timeout: Duration) -> Result<TransportMessage, TransportError> {
        let receiver = self.inbound_rx.lock().unwrap();
        receiver.recv_timeout(timeout).map_err(|err| match err {
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

#[cfg(unix)]
async fn open_stream_pair() -> Result<(UnixStream, UnixStream), TransportError> {
    UnixStream::pair().map_err(to_setup_error)
}

#[cfg(windows)]
async fn open_stream_pair() -> Result<(NamedPipeClient, NamedPipeServer), TransportError> {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static COUNTER: AtomicUsize = AtomicUsize::new(1);
    let id = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let pipe_name = format!(r"\\.\pipe\beach-human-{}", id);

    let server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)
        .map_err(to_setup_error)?;

    let client = ClientOptions::new()
        .open(&pipe_name)
        .map_err(to_setup_error)?;

    server.connect().await.map_err(to_setup_error)?;

    Ok((client, server))
}

async fn create_ipc_pair() -> Result<TransportPair, TransportError> {
    let (client_stream, server_stream) = open_stream_pair().await?;

    let client_id = next_transport_id();
    let server_id = next_transport_id();

    let client_transport =
        IpcTransport::new(TransportKind::Ipc, client_id, server_id, client_stream);
    let server_transport =
        IpcTransport::new(TransportKind::Ipc, server_id, client_id, server_stream);

    Ok(TransportPair {
        client: Box::new(client_transport),
        server: Box::new(server_transport),
    })
}

fn to_setup_error<E: std::fmt::Display>(err: E) -> TransportError {
    TransportError::Setup(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn ipc_pair_round_trip() {
        let pair = build_pair().expect("create ipc pair");
        let timeout = Duration::from_secs(1);

        let client = &pair.client;
        let server = &pair.server;

        assert_eq!(client.kind(), TransportKind::Ipc);
        assert_eq!(server.kind(), TransportKind::Ipc);

        let seq_client = client.send_text("hello from client").expect("client send");
        let seq_server = server.send_text("hello from server").expect("server send");

        let server_msg = server.recv(timeout).expect("server recv");
        assert_eq!(server_msg.sequence, seq_client);
        assert_eq!(server_msg.payload.as_text(), Some("hello from client"));

        let client_msg = client.recv(timeout).expect("client recv");
        assert_eq!(client_msg.sequence, seq_server);
        assert_eq!(client_msg.payload.as_text(), Some("hello from server"));
    }
}

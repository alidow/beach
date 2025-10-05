use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::mcp::McpServerHandle;
use crate::transport::{Payload, Transport, TransportError};

const BRIDGE_BUFFER_SIZE: usize = 64 * 1024;
const TRANSPORT_RECV_TIMEOUT: Duration = Duration::from_secs(30);

pub fn spawn_webrtc_bridge(
    handle: McpServerHandle,
    transport: Arc<dyn Transport>,
    label: &str,
) -> JoinHandle<()> {
    let label = label.to_string();
    tokio::spawn(async move {
        let transport_id = transport.id();
        let peer_id = transport.peer();
        if let Err(err) = run_bridge(handle, transport.clone()).await {
            warn!(
                target = "mcp::bridge",
                transport_id = transport_id.0,
                peer_id = peer_id.0,
                label = %label,
                error = %err,
                "mcp webrtc bridge terminated"
            );
        } else {
            debug!(
                target = "mcp::bridge",
                transport_id = transport_id.0,
                peer_id = peer_id.0,
                label = %label,
                "mcp webrtc bridge completed"
            );
        }
    })
}

async fn run_bridge(handle: McpServerHandle, transport: Arc<dyn Transport>) -> Result<()> {
    let (client_stream, service_stream) = tokio::io::duplex(BRIDGE_BUFFER_SIZE);
    let (client_reader, client_writer) = tokio::io::split(client_stream);
    let mut service_task = handle.spawn_connection(client_reader, client_writer);

    let (service_reader, service_writer) = tokio::io::split(service_stream);
    let mut service_reader = BufReader::new(service_reader);
    let mut service_writer = service_writer;

    let (incoming_tx, mut incoming_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let transport_for_incoming = Arc::clone(&transport);
    let mut incoming_task = tokio::task::spawn_blocking(move || {
        loop {
            match transport_for_incoming.recv(TRANSPORT_RECV_TIMEOUT) {
                Ok(message) => {
                    let payload = match message.payload {
                        Payload::Text(text) => {
                            let mut bytes = text.into_bytes();
                            if !bytes.ends_with(b"\n") {
                                bytes.push(b"\n"[0]);
                            }
                            bytes
                        }
                        Payload::Binary(bytes) => bytes,
                    };
                    if incoming_tx.send(payload).is_err() {
                        break;
                    }
                }
                Err(TransportError::Timeout) => continue,
                Err(TransportError::ChannelClosed) => {
                    let _ = incoming_tx.send(Vec::new());
                    break;
                }
                Err(err) => {
                    warn!(
                        target = "mcp::bridge",
                        error = %err,
                        transport_id = transport_for_incoming.id().0,
                        peer_id = transport_for_incoming.peer().0,
                        "transport recv failed"
                    );
                    let _ = incoming_tx.send(Vec::new());
                    break;
                }
            }
        }
    });

    let mut writer_task = tokio::spawn(async move {
        while let Some(buffer) = incoming_rx.recv().await {
            if buffer.is_empty() {
                break;
            }
            if service_writer.write_all(&buffer).await.is_err() {
                break;
            }
            if service_writer.flush().await.is_err() {
                break;
            }
        }
        let _ = service_writer.shutdown().await;
    });

    let transport_for_outgoing = Arc::clone(&transport);
    let mut outgoing_task = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match service_reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let send_line = line.clone();
                    let transport = Arc::clone(&transport_for_outgoing);
                    let send_result =
                        tokio::task::spawn_blocking(move || transport.send_text(&send_line))
                            .await
                            .map_err(|err| TransportError::Setup(err.to_string()))
                            .and_then(|res| res);
                    if send_result.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = &mut service_task => {}
        _ = &mut incoming_task => {}
        _ = &mut writer_task => {}
        _ = &mut outgoing_task => {}
    }

    if !service_task.is_finished() {
        service_task.abort();
    }
    if !incoming_task.is_finished() {
        incoming_task.abort();
    }
    if !writer_task.is_finished() {
        writer_task.abort();
    }
    if !outgoing_task.is_finished() {
        outgoing_task.abort();
    }

    Ok(())
}

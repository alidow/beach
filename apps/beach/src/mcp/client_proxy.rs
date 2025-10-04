use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::transport::{Transport, TransportError};

const TRANSPORT_RECV_TIMEOUT: Duration = Duration::from_secs(30);

pub fn spawn_client_proxy(path: PathBuf, transport: Arc<dyn Transport>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(parent) = path.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                warn!(error = %err, dir = %parent.display(), "failed to create mcp proxy directory");
                return;
            }
        }
        if tokio::fs::metadata(&path).await.is_ok() {
            if let Err(err) = tokio::fs::remove_file(&path).await {
                warn!(error = %err, path = %path.display(), "failed to remove existing mcp proxy socket");
                return;
            }
        }

        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(err) => {
                warn!(error = %err, path = %path.display(), "failed to bind mcp proxy socket");
                return;
            }
        };

        debug!(path = %path.display(), "mcp proxy listening");

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    if let Err(err) = proxy_connection(stream, Arc::clone(&transport)).await {
                        warn!(error = %err, "mcp proxy connection terminated");
                    }
                }
                Err(err) => {
                    warn!(error = %err, path = %path.display(), "mcp proxy accept failed");
                    break;
                }
            }
        }
    })
}

async fn proxy_connection(stream: UnixStream, transport: Arc<dyn Transport>) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut writer = writer;
    let mut reader = BufReader::new(reader);

    let (incoming_tx, mut incoming_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let transport_for_incoming = Arc::clone(&transport);
    let mut incoming_task = tokio::task::spawn_blocking(move || {
        loop {
            match transport_for_incoming.recv(TRANSPORT_RECV_TIMEOUT) {
                Ok(message) => {
                    let payload = match message.payload {
                        crate::transport::Payload::Text(text) => {
                            let mut bytes = text.into_bytes();
                            if !bytes.ends_with(b"\n") {
                                bytes.push(b"\n"[0]);
                            }
                            bytes
                        }
                        crate::transport::Payload::Binary(bytes) => bytes,
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
                    warn!(error = %err, "mcp proxy transport recv failed");
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
            if writer.write_all(&buffer).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });

    let transport_for_outgoing = Arc::clone(&transport);
    let mut outgoing_task = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let send_line = line.clone();
                    let transport = Arc::clone(&transport_for_outgoing);
                    let send_result =
                        tokio::task::spawn_blocking(move || transport.send_text(&send_line))
                            .await
                            .map_err(|err| TransportError::Setup(err.to_string()))
                            .and_then(|res| res);
                    if let Err(err) = send_result {
                        warn!(error = %err, "mcp proxy send failed");
                        break;
                    }
                }
                Err(err) => {
                    warn!(error = %err, "mcp proxy read failed");
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = &mut incoming_task => {}
        _ = &mut writer_task => {}
        _ = &mut outgoing_task => {}
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

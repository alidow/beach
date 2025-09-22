use super::spawn_on_global;
use crate::transport::TransportError;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, Notify, RwLock, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;
use uuid::Uuid;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;

use super::WebRtcRole;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    #[serde(rename = "webrtc")]
    WebRTC,
    WebTransport,
    Direct,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal_type", rename_all = "snake_case")]
pub enum WebRTCSignal {
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportSignal {
    #[serde(rename = "webrtc")]
    WebRTC { signal: WebRTCSignal },
}

impl TransportSignal {
    pub fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    pub fn from_value(value: &Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }
}

impl WebRTCSignal {
    pub fn to_transport_signal(self) -> TransportSignal {
        TransportSignal::WebRTC { signal: self }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub role: PeerRole,
    pub joined_at: i64,
    pub supported_transports: Vec<TransportType>,
    pub preferred_transport: Option<TransportType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        peer_id: String,
        passphrase: Option<String>,
        supported_transports: Vec<TransportType>,
        preferred_transport: Option<TransportType>,
    },
    Signal {
        to_peer: String,
        signal: Value,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    JoinSuccess {
        session_id: String,
        peer_id: String,
        peers: Vec<PeerInfo>,
        available_transports: Vec<TransportType>,
    },
    JoinError {
        reason: String,
    },
    PeerJoined {
        peer: PeerInfo,
    },
    PeerLeft {
        peer_id: String,
    },
    Signal {
        from_peer: String,
        signal: Value,
    },
    Pong,
    Error {
        message: String,
    },
}

pub struct SignalingClient {
    peer_id: String,
    expected_remote_role: PeerRole,
    send_tx: mpsc::UnboundedSender<ClientMessage>,
    signal_rx: AsyncMutex<mpsc::UnboundedReceiver<WebRTCSignal>>,
    remote_peer: RwLock<Option<String>>,
    remote_notify: Notify,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl SignalingClient {
    pub async fn connect(
        signaling_url: &str,
        role: WebRtcRole,
        passphrase: Option<&str>,
    ) -> Result<Arc<Self>, TransportError> {
        let websocket_url = derive_websocket_url(signaling_url)?;
        let (ws_stream, _) = connect_async(websocket_url.as_str())
            .await
            .map_err(|err| TransportError::Setup(format!("websocket connect failed: {err}")))?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let (signal_tx, signal_rx) = mpsc::unbounded_channel::<WebRTCSignal>();

        let client = Arc::new(SignalingClient {
            peer_id: Uuid::new_v4().to_string(),
            expected_remote_role: expected_remote_role(role),
            send_tx,
            signal_rx: AsyncMutex::new(signal_rx),
            remote_peer: RwLock::new(None),
            remote_notify: Notify::new(),
            tasks: Mutex::new(Vec::new()),
        });

        let (join_tx, join_rx) = tokio::sync::oneshot::channel::<()>();
        let join_notifier = Arc::new(tokio::sync::Mutex::new(Some(join_tx)));

        let writer_handle = spawn_on_global(async move {
            while let Some(message) = send_rx.recv().await {
                if let Ok(text) = serde_json::to_string(&message) {
                    if ws_write.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
            }
        });

        let reader_client = Arc::clone(&client);
        let reader_join = Arc::clone(&join_notifier);
        let signal_tx_clone = signal_tx.clone();
        let reader_handle = spawn_on_global(async move {
            while let Some(msg) = ws_read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                            handle_server_message(
                                &reader_client,
                                server_msg,
                                &signal_tx_clone,
                                &reader_join,
                            )
                            .await;
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        if let Ok(text) = String::from_utf8(data) {
                            if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                                handle_server_message(
                                    &reader_client,
                                    server_msg,
                                    &signal_tx_clone,
                                    &reader_join,
                                )
                                .await;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(target = "webrtc", "signaling websocket error: {err}");
                        break;
                    }
                }
            }
        });

        let heartbeat_client = Arc::clone(&client);
        let heartbeat_handle = spawn_on_global(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                if heartbeat_client.send_tx.send(ClientMessage::Ping).is_err() {
                    break;
                }
            }
        });

        {
            let mut guard = client.tasks.lock().unwrap();
            guard.push(writer_handle);
            guard.push(reader_handle);
            guard.push(heartbeat_handle);
        }

        let join_message = ClientMessage::Join {
            peer_id: client.peer_id.clone(),
            passphrase: passphrase.map(|s| s.to_string()),
            supported_transports: vec![TransportType::WebRTC],
            preferred_transport: Some(TransportType::WebRTC),
        };
        client
            .send_tx
            .send(join_message)
            .map_err(|_| TransportError::ChannelClosed)?;

        match join_rx.await {
            Ok(()) => Ok(client),
            Err(_) => {
                tracing::warn!(target = "webrtc", "signaling join channel dropped");
                Err(TransportError::ChannelClosed)
            }
        }
    }

    pub async fn wait_for_remote_peer(&self) -> Result<String, TransportError> {
        loop {
            if let Some(id) = self.remote_peer.read().await.clone() {
                return Ok(id);
            }
            self.remote_notify.notified().await;
        }
    }

    pub async fn send_ice_candidate(
        &self,
        candidate: RTCIceCandidate,
    ) -> Result<(), TransportError> {
        let json = candidate
            .to_json()
            .map_err(|err| TransportError::Setup(err.to_string()))?;
        let signal = WebRTCSignal::IceCandidate {
            candidate: json.candidate,
            sdp_mid: json.sdp_mid,
            sdp_mline_index: json.sdp_mline_index.map(|idx| idx as u32),
        };
        self.send_webrtc_signal(signal).await
    }

    pub async fn send_webrtc_signal(&self, signal: WebRTCSignal) -> Result<(), TransportError> {
        let remote = self.wait_for_remote_peer().await?;
        let payload = signal
            .to_transport_signal()
            .to_value()
            .map_err(|err| TransportError::Setup(err.to_string()))?;
        self.send_tx
            .send(ClientMessage::Signal {
                to_peer: remote,
                signal: payload,
            })
            .map_err(|_| TransportError::ChannelClosed)
    }

    pub async fn recv_webrtc_signal(&self) -> Option<WebRTCSignal> {
        let mut rx = self.signal_rx.lock().await;
        rx.recv().await
    }

    async fn set_remote_peer(&self, peer_id: String) {
        let mut guard = self.remote_peer.write().await;
        if guard.as_ref() != Some(&peer_id) {
            *guard = Some(peer_id);
            self.remote_notify.notify_waiters();
        }
    }
}

impl Drop for SignalingClient {
    fn drop(&mut self) {
        if let Ok(mut tasks) = self.tasks.lock() {
            for handle in tasks.drain(..) {
                handle.abort();
            }
        }
    }
}

async fn handle_server_message(
    client: &Arc<SignalingClient>,
    message: ServerMessage,
    signal_tx: &mpsc::UnboundedSender<WebRTCSignal>,
    join_notifier: &Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
) {
    match message {
        ServerMessage::JoinSuccess { peers, .. } => {
            if let Some(target) = peers
                .into_iter()
                .find(|peer| peer.id != client.peer_id && peer.role == client.expected_remote_role)
            {
                client.set_remote_peer(target.id).await;
            }
            if let Some(tx) = join_notifier.lock().await.take() {
                let _ = tx.send(());
            }
        }
        ServerMessage::PeerJoined { peer } => {
            if peer.role == client.expected_remote_role {
                client.set_remote_peer(peer.id).await;
            }
        }
        ServerMessage::Signal { signal, .. } => {
            if let Ok(TransportSignal::WebRTC { signal }) = TransportSignal::from_value(&signal) {
                let _ = signal_tx.send(signal);
            }
        }
        _ => {}
    }
}

fn expected_remote_role(role: WebRtcRole) -> PeerRole {
    match role {
        WebRtcRole::Offerer => PeerRole::Client,
        WebRtcRole::Answerer => PeerRole::Server,
    }
}

fn derive_websocket_url(signaling_url: &str) -> Result<Url, TransportError> {
    let base = Url::parse(signaling_url).map_err(|err| {
        TransportError::Setup(format!("invalid signaling url {signaling_url}: {err}"))
    })?;
    let segments = base
        .path_segments()
        .ok_or_else(|| TransportError::Setup("signaling url missing path segments".into()))?
        .collect::<Vec<_>>();
    if segments.len() < 3 || segments[0] != "sessions" {
        return Err(TransportError::Setup(format!(
            "unexpected signaling url path: {}",
            base.path()
        )));
    }
    let session_id = segments[1];

    let mut ws = base.clone();
    ws.set_scheme(if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    })
    .map_err(|_| TransportError::Setup("invalid websocket scheme".into()))?;
    ws.set_path(&format!("ws/{session_id}"));
    ws.set_query(None);
    ws.set_fragment(None);
    Ok(ws)
}

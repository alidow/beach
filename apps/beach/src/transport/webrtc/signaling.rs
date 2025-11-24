use super::IceCandidateBlob;
use super::secure_signaling::{
    MessageLabel, SealedEnvelope, seal_message, seal_message_with_psk, should_encrypt,
};
use super::spawn_on_global;
use crate::transport::TransportError;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, Notify, RwLock, mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Error as WsError, Message, error::ProtocolError},
};
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
        handshake_id: String,
    },
    Answer {
        sdp: String,
        handshake_id: String,
    },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
        handshake_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sealed: Option<SealedEnvelope>,
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
    pub fn into_transport_signal(self) -> TransportSignal {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug)]
pub struct RemotePeerJoined {
    pub peer: PeerInfo,
    pub generation: u64,
    pub signals: mpsc::UnboundedReceiver<WebRTCSignal>,
}

#[derive(Debug, Clone)]
pub struct RemotePeerLeft {
    pub peer_id: String,
    pub generation: u64,
}

#[derive(Debug)]
pub enum RemotePeerEvent {
    Joined(RemotePeerJoined),
    Left(RemotePeerLeft),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        peer_id: String,
        passphrase: Option<String>,
        supported_transports: Vec<TransportType>,
        preferred_transport: Option<TransportType>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        mcp: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<HashMap<String, String>>,
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
    remote_metadata: RwLock<Option<HashMap<String, String>>>,
    remote_notify: Notify,
    locked_peer: RwLock<Option<String>>,
    remote_generation: AtomicU64,
    assigned_peer_id: RwLock<Option<String>>,
    peer_generation_counter: AtomicU64,
    peer_channels: RwLock<HashMap<String, PeerChannelEntry>>,
    remote_events_tx: mpsc::UnboundedSender<RemotePeerEvent>,
    remote_events_rx: AsyncMutex<Option<mpsc::UnboundedReceiver<RemotePeerEvent>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    passphrase: Option<String>,
}

#[derive(Clone)]
struct PeerChannelEntry {
    sender: mpsc::UnboundedSender<WebRTCSignal>,
    generation: u64,
}

impl SignalingClient {
    pub async fn connect(
        signaling_url: &str,
        peer_id: Option<String>,
        role: WebRtcRole,
        passphrase: Option<&str>,
        label: Option<String>,
        request_mcp_channel: bool,
        metadata: Option<HashMap<String, String>>,
    ) -> Result<Arc<Self>, TransportError> {
        let websocket_url = derive_websocket_url(signaling_url)?;
        let (ws_stream, _) = connect_async(websocket_url.as_str())
            .await
            .map_err(|err| TransportError::Setup(format!("websocket connect failed: {err}")))?;
        tracing::debug!(
            target = "webrtc",
            url = %websocket_url,
            "signaling websocket connected"
        );
        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<ClientMessage>();
        let (signal_tx, signal_rx) = mpsc::unbounded_channel::<WebRTCSignal>();
        let (remote_events_tx, remote_events_rx) = mpsc::unbounded_channel::<RemotePeerEvent>();
        let passphrase_owned = passphrase.map(|s| s.to_string());
        let assigned_peer_id = peer_id.unwrap_or_else(|| Uuid::new_v4().to_string());

        let client = Arc::new(SignalingClient {
            peer_id: assigned_peer_id,
            expected_remote_role: expected_remote_role(role),
            send_tx,
            signal_rx: AsyncMutex::new(signal_rx),
            remote_peer: RwLock::new(None),
            remote_metadata: RwLock::new(None),
            remote_notify: Notify::new(),
            locked_peer: RwLock::new(None),
            remote_generation: AtomicU64::new(0),
            assigned_peer_id: RwLock::new(None),
            peer_generation_counter: AtomicU64::new(0),
            peer_channels: RwLock::new(HashMap::new()),
            remote_events_tx,
            remote_events_rx: AsyncMutex::new(Some(remote_events_rx)),
            tasks: Mutex::new(Vec::new()),
            passphrase: passphrase_owned.clone(),
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
                        tracing::trace!(
                            target = "webrtc",
                            event = "ws_in",
                            kind = "text",
                            len = text.len()
                        );
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
                        tracing::trace!(
                            target = "webrtc",
                            event = "ws_in",
                            kind = "binary",
                            len = data.len()
                        );
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
                        match &err {
                            WsError::ConnectionClosed
                            | WsError::AlreadyClosed
                            | WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
                                tracing::debug!(
                                    target = "webrtc",
                                    "signaling websocket closed: {err}"
                                );
                            }
                            _ => {
                                tracing::warn!(
                                    target = "webrtc",
                                    "signaling websocket error: {err}"
                                );
                            }
                        }
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
            passphrase: passphrase_owned,
            supported_transports: vec![TransportType::WebRTC],
            preferred_transport: Some(TransportType::WebRTC),
            label,
            mcp: request_mcp_channel,
            metadata,
        };
        client
            .send_tx
            .send(join_message)
            .map_err(|_| TransportError::ChannelClosed)?;
        tracing::debug!(target = "webrtc", "sent signaling join message");

        match join_rx.await {
            Ok(()) => {
                tracing::debug!(target = "webrtc", "received signaling join_success");
                Ok(client)
            }
            Err(_) => {
                tracing::warn!(target = "webrtc", "signaling join channel dropped");
                Err(TransportError::ChannelClosed)
            }
        }
    }

    pub fn remote_generation(&self) -> u64 {
        self.remote_generation.load(Ordering::SeqCst)
    }

    pub async fn assigned_peer_id(&self) -> Option<String> {
        self.assigned_peer_id.read().await.clone()
    }

    pub async fn remote_events(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<RemotePeerEvent>, TransportError> {
        let mut guard = self.remote_events_rx.lock().await;
        guard
            .take()
            .ok_or_else(|| TransportError::Setup("remote event stream already taken".into()))
    }

    pub async fn remote_metadata(&self) -> Option<HashMap<String, String>> {
        self.remote_metadata.read().await.clone()
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    async fn is_self_peer(&self, peer_id: &str) -> bool {
        if peer_id == self.peer_id {
            return true;
        }
        let guard = self.assigned_peer_id.read().await;
        guard.as_deref() == Some(peer_id)
    }

    pub async fn wait_for_remote_peer_with_generation(
        &self,
    ) -> Result<(String, u64), TransportError> {
        loop {
            let generation = self.remote_generation.load(Ordering::SeqCst);
            if let Some(id) = self.remote_peer.read().await.clone() {
                if generation == self.remote_generation.load(Ordering::SeqCst) {
                    return Ok((id, generation));
                }
            }
            self.remote_notify.notified().await;
        }
    }

    #[allow(dead_code)]
    pub async fn wait_for_remote_peer(&self) -> Result<String, TransportError> {
        let (peer, _) = self.wait_for_remote_peer_with_generation().await?;
        Ok(peer)
    }

    pub async fn lock_remote_peer(&self, peer_id: &str) {
        let mut lock = self.locked_peer.write().await;
        *lock = Some(peer_id.to_string());
    }

    pub async fn unlock_remote_peer(&self, peer_id: &str) {
        let mut lock = self.locked_peer.write().await;
        if lock.as_deref() == Some(peer_id) {
            *lock = None;
        }
    }

    pub async fn send_ice_candidate(
        &self,
        candidate: RTCIceCandidate,
        handshake_id: &str,
        pre_shared_key: Option<Arc<[u8; 32]>>,
    ) -> Result<(), TransportError> {
        let (remote, _) = self.wait_for_remote_peer_with_generation().await?;
        self.send_ice_candidate_to_peer(candidate, handshake_id, &remote, pre_shared_key)
            .await
    }

    pub async fn send_ice_candidate_to_peer(
        &self,
        candidate: RTCIceCandidate,
        handshake_id: &str,
        peer_id: &str,
        pre_shared_key: Option<Arc<[u8; 32]>>,
    ) -> Result<(), TransportError> {
        let json = candidate
            .to_json()
            .map_err(|err| TransportError::Setup(err.to_string()))?;
        let blob = IceCandidateBlob {
            candidate: json.candidate,
            sdp_mid: json.sdp_mid.clone(),
            sdp_mline_index: json.sdp_mline_index.map(|idx| idx as u32),
        };
        let mut candidate_text = blob.candidate.clone();
        let mut sdp_mid = blob.sdp_mid.clone();
        let mut sdp_mline_index = blob.sdp_mline_index;
        let mut sealed = None;

        if let Some(passphrase) = self.passphrase.as_deref() {
            if should_encrypt(Some(passphrase)) {
                let (local_peer_id, assigned_ready) = match self.assigned_peer_id().await {
                    Some(id) => (id, true),
                    None => (self.peer_id.clone(), false),
                };
                if !assigned_ready {
                    tracing::warn!(
                        target = "webrtc",
                        handshake_id = %handshake_id,
                        provisional_peer = %local_peer_id,
                        target_peer = %peer_id,
                        "sealing ice candidate with provisional peer id (assigned id not yet available)"
                    );
                }
                let plaintext = serde_json::to_vec(&blob).map_err(|err| {
                    TransportError::Setup(format!("serialize ice candidate failed: {err}"))
                })?;
                let associated = [local_peer_id.as_str(), peer_id, handshake_id];
                if let Some(psk) = pre_shared_key.as_deref() {
                    tracing::info!(
                        target = "webrtc",
                        handshake_id = %handshake_id,
                        aad_from = %local_peer_id,
                        aad_to = %peer_id,
                        aad_label = "ice",
                        using_assigned_id = assigned_ready,
                        "sealing ice candidate with handshake key"
                    );
                    match seal_message_with_psk(
                        psk,
                        handshake_id,
                        MessageLabel::Ice,
                        &associated,
                        &plaintext,
                    ) {
                        Ok(envelope) => {
                            tracing::info!(
                                target = "webrtc",
                                handshake_id = %handshake_id,
                                ciphertext_len = envelope.ciphertext.len(),
                                nonce = envelope.nonce.as_str(),
                                using_assigned_id = assigned_ready,
                                "sealed ice candidate created with handshake key"
                            );
                            sealed = Some(envelope);
                        }
                        Err(err) => {
                            tracing::warn!(
                                target = "webrtc",
                                handshake_id = %handshake_id,
                                error = %err,
                                "failed to seal ice candidate with handshake key; falling back to passphrase"
                            );
                        }
                    }
                } else {
                    tracing::debug!(
                        target = "webrtc",
                        handshake_id = %handshake_id,
                        aad_from = %local_peer_id,
                        aad_to = %peer_id,
                        aad_label = "ice",
                        using_assigned_id = assigned_ready,
                        "handshake key unavailable; sealing ice candidate with passphrase"
                    );
                }
                if sealed.is_none() {
                    let envelope = seal_message(
                        passphrase,
                        handshake_id,
                        MessageLabel::Ice,
                        &associated,
                        &plaintext,
                    )?;
                    tracing::info!(
                        target = "webrtc",
                        handshake_id = %handshake_id,
                        ciphertext_len = envelope.ciphertext.len(),
                        nonce = envelope.nonce.as_str(),
                        using_assigned_id = assigned_ready,
                        "sealed ice candidate created with passphrase key"
                    );
                    sealed = Some(envelope);
                }
                if sealed.is_some() {
                    candidate_text.clear();
                    sdp_mid = None;
                    sdp_mline_index = None;
                }
            }
        }

        let signal = WebRTCSignal::IceCandidate {
            candidate: candidate_text,
            sdp_mid,
            sdp_mline_index,
            handshake_id: handshake_id.to_string(),
            sealed,
        };
        self.send_signal_to_peer(peer_id, signal).await
    }

    pub async fn send_signal_to_peer(
        &self,
        peer_id: &str,
        signal: WebRTCSignal,
    ) -> Result<(), TransportError> {
        let payload = signal
            .into_transport_signal()
            .to_value()
            .map_err(|err| TransportError::Setup(err.to_string()))?;
        self.send_tx
            .send(ClientMessage::Signal {
                to_peer: peer_id.to_string(),
                signal: payload,
            })
            .map_err(|_| TransportError::ChannelClosed)
    }

    pub async fn recv_webrtc_signal(&self) -> Option<WebRTCSignal> {
        let mut rx = self.signal_rx.lock().await;
        rx.recv().await
    }

    async fn register_client_peer(&self, peer: PeerInfo) {
        // For non-client expected roles (e.g., Answerer expecting Server),
        // prioritize role matching before self-check. This avoids a race where
        // an incorrect assigned_peer_id equal to the server's id causes us to
        // treat the remote server as "self" and never set the remote peer.
        if self.expected_remote_role != PeerRole::Client {
            if peer.role == self.expected_remote_role {
                self.set_remote_peer(peer.id.clone()).await;
                self.set_remote_metadata(peer.metadata.clone()).await;
            }
            return;
        }

        if self.is_self_peer(&peer.id).await {
            return;
        }

        let mut channels = self.peer_channels.write().await;
        if channels.contains_key(&peer.id) {
            return;
        }
        let generation = self.peer_generation_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let peer_id = peer.id.clone();
        channels.insert(
            peer_id.clone(),
            PeerChannelEntry {
                sender: tx,
                generation,
            },
        );
        drop(channels);

        let join_event = RemotePeerEvent::Joined(RemotePeerJoined {
            peer,
            generation,
            signals: rx,
        });

        if self.remote_events_tx.send(join_event).is_err() {
            tracing::debug!(
                target = "webrtc",
                peer_id = %peer_id,
                "remote events channel closed; dropping join event"
            );
            let mut channels = self.peer_channels.write().await;
            channels.remove(&peer_id);
        }
    }

    async fn unregister_client_peer(&self, peer_id: &str) {
        if self.expected_remote_role != PeerRole::Client {
            self.clear_remote_peer(peer_id).await;
            return;
        }

        let removed = {
            let mut channels = self.peer_channels.write().await;
            channels.remove(peer_id)
        };

        if let Some(entry) = removed {
            if self
                .remote_events_tx
                .send(RemotePeerEvent::Left(RemotePeerLeft {
                    peer_id: peer_id.to_string(),
                    generation: entry.generation,
                }))
                .is_err()
            {
                tracing::debug!(
                    target = "webrtc",
                    peer_id = %peer_id,
                    "remote events channel closed; dropping leave event"
                );
            }
        }

        self.clear_remote_peer(peer_id).await;
    }

    async fn forward_signal_to_client(&self, peer_id: &str, signal: WebRTCSignal) -> bool {
        if self.expected_remote_role != PeerRole::Client {
            return false;
        }

        let entry = {
            let channels = self.peer_channels.read().await;
            channels.get(peer_id).cloned()
        };
        if let Some(entry) = entry {
            if entry.sender.send(signal).is_err() {
                tracing::debug!(
                    target = "webrtc",
                    peer_id = %peer_id,
                    "peer signal channel closed; removing peer"
                );
                self.unregister_client_peer(peer_id).await;
            }
            true
        } else {
            false
        }
    }

    async fn set_remote_peer(&self, peer_id: String) {
        {
            let lock_guard = self.locked_peer.read().await;
            if let Some(ref locked) = *lock_guard {
                if locked != &peer_id {
                    return;
                }
            }
        }
        let mut guard = self.remote_peer.write().await;
        if guard.as_ref() != Some(&peer_id) {
            let previous = guard.clone();
            *guard = Some(peer_id.clone());
            let generation = self.remote_generation.fetch_add(1, Ordering::SeqCst) + 1;
            self.remote_notify.notify_waiters();
            match previous {
                Some(old) => tracing::info!(
                    target = "webrtc",
                    previous = %old,
                    current = %peer_id,
                    generation,
                    "reassigned remote peer"
                ),
                None => tracing::info!(
                    target = "webrtc",
                    current = %peer_id,
                    generation,
                    "selected remote peer"
                ),
            }
        }
    }

    async fn clear_remote_peer(&self, peer_id: &str) {
        let unlock_ok = {
            let lock_guard = self.locked_peer.read().await;
            lock_guard.as_deref() == Some(peer_id)
        };

        let mut guard = self.remote_peer.write().await;
        let cleared = if guard.as_deref() == Some(peer_id) {
            *guard = None;
            let generation = self.remote_generation.fetch_add(1, Ordering::SeqCst) + 1;
            self.remote_notify.notify_waiters();
            tracing::info!(
                target = "webrtc",
                peer = %peer_id,
                generation,
                "cleared remote peer"
            );
            true
        } else {
            false
        };
        drop(guard);

        if unlock_ok {
            let mut lock = self.locked_peer.write().await;
            if lock.as_deref() == Some(peer_id) {
                *lock = None;
            }
        }

        if cleared {
            self.set_remote_metadata(None).await;
        }
    }

    async fn set_remote_metadata(&self, metadata: Option<HashMap<String, String>>) {
        let mut guard = self.remote_metadata.write().await;
        *guard = metadata;
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
        ServerMessage::JoinSuccess {
            peer_id: assigned_id,
            peers,
            ..
        } => {
            tracing::debug!(
                target = "webrtc",
                assigned_peer = %assigned_id,
                peers = peers.len(),
                "signaling join_success received"
            );
            *client.assigned_peer_id.write().await = Some(assigned_id.clone());
            for peer in peers {
                tracing::trace!(
                    target = "webrtc",
                    peer_id = %peer.id,
                    role = ?peer.role,
                    "signaling known peer"
                );
                if peer.role != client.expected_remote_role {
                    continue;
                }
                client.register_client_peer(peer).await;
            }
            if let Some(tx) = join_notifier.lock().await.take() {
                let _ = tx.send(());
            }
        }
        ServerMessage::PeerJoined { peer } => {
            client.register_client_peer(peer).await;
        }
        ServerMessage::PeerLeft { peer_id } => {
            client.unregister_client_peer(&peer_id).await;
        }
        ServerMessage::Signal { from_peer, signal } => {
            if client.expected_remote_role != PeerRole::Client
                && !client.is_self_peer(&from_peer).await
            {
                client.set_remote_peer(from_peer.clone()).await;
            }
            if let Ok(TransportSignal::WebRTC { signal }) = TransportSignal::from_value(&signal) {
                let routed = client
                    .forward_signal_to_client(&from_peer, signal.clone())
                    .await;
                if !routed {
                    let _ = signal_tx.send(signal);
                } else if signal_tx.send(signal).is_err() {
                    tracing::debug!(
                        target = "webrtc",
                        peer_id = %from_peer,
                        "global signaling channel closed"
                    );
                }
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
    let session_id = if segments.len() >= 3 && segments[0] == "sessions" {
        segments[1]
    } else if segments.len() >= 3 && segments[0] == "peer-sessions" {
        segments[1]
    } else {
        return Err(TransportError::Setup(format!(
            "unexpected signaling url path: {}",
            base.path()
        )));
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_client(expected_remote_role: PeerRole) -> Arc<SignalingClient> {
        let (send_tx, _) = mpsc::unbounded_channel();
        let (_signal_tx, signal_rx) = mpsc::unbounded_channel();
        let (remote_events_tx, remote_events_rx) = mpsc::unbounded_channel();
        Arc::new(SignalingClient {
            peer_id: "peer".into(),
            expected_remote_role,
            send_tx,
            signal_rx: AsyncMutex::new(signal_rx),
            remote_peer: RwLock::new(None),
            remote_metadata: RwLock::new(None),
            remote_notify: Notify::new(),
            locked_peer: RwLock::new(None),
            remote_generation: AtomicU64::new(0),
            assigned_peer_id: RwLock::new(None),
            peer_generation_counter: AtomicU64::new(0),
            peer_channels: RwLock::new(HashMap::new()),
            remote_events_tx,
            remote_events_rx: AsyncMutex::new(Some(remote_events_rx)),
            tasks: Mutex::new(Vec::new()),
            passphrase: None,
        })
    }

    #[tokio::test]
    async fn register_client_peer_stores_metadata_for_non_client_roles() {
        let client = make_client(PeerRole::Server);
        let mut metadata = HashMap::new();
        metadata.insert("label".to_string(), "pb-controller".to_string());
        let peer = PeerInfo {
            id: "server-1".into(),
            role: PeerRole::Server,
            joined_at: 0,
            supported_transports: vec![],
            preferred_transport: None,
            metadata: Some(metadata.clone()),
        };

        client.register_client_peer(peer).await;

        assert_eq!(client.remote_metadata().await, Some(metadata));
    }
}

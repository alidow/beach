use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use crossbeam_channel::{
    Receiver as CrossbeamReceiver, RecvTimeoutError as CrossbeamRecvTimeoutError,
    TryRecvError as CrossbeamTryRecvError, unbounded as crossbeam_unbounded,
};
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc as tokio_mpsc, oneshot};
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{API, APIBuilder};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::util::vnet::net::{Net, NetConfig};
use webrtc::util::vnet::router::{Router, RouterConfig};

use crate::transport::{
    Transport, TransportError, TransportId, TransportKind, TransportMessage, TransportPair,
    decode_message, encode_message, next_transport_id,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_ACK_POLL_ATTEMPTS: usize = 200;
const READY_ACK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MCP_CHANNEL_LABEL: &str = "mcp-jsonrpc";
mod secure_handshake;
mod secure_signaling;
mod signaling;

use secure_handshake::{
    HANDSHAKE_CHANNEL_LABEL, HandshakeParams, HandshakeResult, HandshakeRole,
    build_prologue_context, handshake_channel_init, run_handshake, secure_transport_enabled,
};
use secure_signaling::{
    MessageLabel, SealedEnvelope, derive_pre_shared_key, open_message, seal_message, should_encrypt,
};
use signaling::{PeerRole, RemotePeerEvent, RemotePeerJoined, SignalingClient, WebRTCSignal};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct IceCandidateBlob {
    candidate: String,
    sdp_mid: Option<String>,
    sdp_mline_index: Option<u32>,
}

#[derive(Clone, Default)]
pub struct WebRtcChannels {
    inner: Arc<WebRtcChannelsInner>,
}

#[derive(Default)]
#[allow(clippy::type_complexity)]
struct WebRtcChannelsInner {
    channels: Mutex<HashMap<String, Arc<dyn Transport>>>,
    waiters: Mutex<HashMap<String, Vec<oneshot::Sender<Arc<dyn Transport>>>>>,
}

impl WebRtcChannels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, label: String, transport: Arc<dyn Transport>) {
        {
            let mut guard = self.inner.channels.lock().unwrap();
            guard.insert(label.clone(), transport.clone());
        }
        let waiters = {
            let mut guard = self.inner.waiters.lock().unwrap();
            guard.remove(&label)
        };
        if let Some(waiters) = waiters {
            for waiter in waiters {
                let _ = waiter.send(transport.clone());
            }
        }
    }

    pub fn try_get(&self, label: &str) -> Option<Arc<dyn Transport>> {
        let guard = self.inner.channels.lock().unwrap();
        guard.get(label).cloned()
    }

    pub async fn wait_for(&self, label: &str) -> Result<Arc<dyn Transport>, TransportError> {
        if let Some(existing) = self.try_get(label) {
            return Ok(existing);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.inner.waiters.lock().unwrap();
            guard.entry(label.to_string()).or_default().push(tx);
        }
        rx.await.map_err(|_| TransportError::ChannelClosed)
    }
}

const TRANSPORT_ENCRYPTION_VERSION: u8 = 1;
const TRANSPORT_ENCRYPTION_AAD: &[u8] = b"beach:secure-transport:v1";

struct EncryptionState {
    send_cipher: ChaCha20Poly1305,
    recv_cipher: ChaCha20Poly1305,
    send_counter: AtomicU64,
    recv_counter: AtomicU64,
}

struct EncryptionManager {
    state: Mutex<Option<EncryptionState>>,
    enabled: AtomicBool,
}

impl EncryptionManager {
    fn new() -> Self {
        Self {
            state: Mutex::new(None),
            enabled: AtomicBool::new(false),
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn enable(&self, keys: &HandshakeResult) -> Result<(), TransportError> {
        let mut guard = self.state.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
        let send_cipher = ChaCha20Poly1305::new_from_slice(&keys.send_key).map_err(|err| {
            TransportError::Setup(format!("secure transport send key invalid: {err}"))
        })?;
        let recv_cipher = ChaCha20Poly1305::new_from_slice(&keys.recv_key).map_err(|err| {
            TransportError::Setup(format!("secure transport recv key invalid: {err}"))
        })?;
        guard.replace(EncryptionState {
            send_cipher,
            recv_cipher,
            send_counter: AtomicU64::new(0),
            recv_counter: AtomicU64::new(0),
        });
        self.enabled.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, TransportError> {
        let guard = self.state.lock().unwrap();
        let state = guard
            .as_ref()
            .ok_or_else(|| TransportError::Setup("secure transport not negotiated".into()))?;
        let counter = state.send_counter.fetch_add(1, Ordering::SeqCst);
        let nonce_bytes = nonce_from_counter(counter);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = state
            .send_cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: TRANSPORT_ENCRYPTION_AAD,
                },
            )
            .map_err(|err| {
                TransportError::Setup(format!("secure transport encrypt failed: {err}"))
            })?;
        let mut frame = Vec::with_capacity(1 + 8 + ciphertext.len());
        frame.push(TRANSPORT_ENCRYPTION_VERSION);
        frame.extend_from_slice(&counter.to_be_bytes());
        frame.extend_from_slice(&ciphertext);
        Ok(frame)
    }

    fn decrypt(&self, frame: &[u8]) -> Result<Vec<u8>, TransportError> {
        let guard = self.state.lock().unwrap();
        let state = guard
            .as_ref()
            .ok_or_else(|| TransportError::Setup("secure transport not negotiated".into()))?;
        if frame.len() < 9 {
            return Err(TransportError::Setup(
                "secure transport frame too short".into(),
            ));
        }
        let version = frame[0];
        if version != TRANSPORT_ENCRYPTION_VERSION {
            return Err(TransportError::Setup(
                "secure transport version mismatch".into(),
            ));
        }
        let mut counter_bytes = [0u8; 8];
        counter_bytes.copy_from_slice(&frame[1..9]);
        let counter = u64::from_be_bytes(counter_bytes);
        let expected = state.recv_counter.load(Ordering::SeqCst);
        if counter != expected {
            return Err(TransportError::Setup(
                "secure transport counter mismatch".into(),
            ));
        }
        let nonce_bytes = nonce_from_counter(counter);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let plaintext = state
            .recv_cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &frame[9..],
                    aad: TRANSPORT_ENCRYPTION_AAD,
                },
            )
            .map_err(|err| {
                TransportError::Setup(format!("secure transport decrypt failed: {err}"))
            })?;
        state.recv_counter.fetch_add(1, Ordering::SeqCst);
        Ok(plaintext)
    }
}

fn nonce_from_counter(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

#[derive(Clone)]
pub struct WebRtcConnection {
    transport: Arc<dyn Transport>,
    channels: WebRtcChannels,
    secure: Option<Arc<HandshakeResult>>,
}

impl WebRtcConnection {
    pub fn new(
        transport: Arc<dyn Transport>,
        channels: WebRtcChannels,
        secure: Option<Arc<HandshakeResult>>,
    ) -> Self {
        Self {
            transport,
            channels,
            secure,
        }
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        self.transport.clone()
    }

    pub fn channels(&self) -> WebRtcChannels {
        self.channels.clone()
    }

    pub fn secure(&self) -> Option<Arc<HandshakeResult>> {
        self.secure.clone()
    }
}

pub fn build_pair() -> Result<TransportPair, TransportError> {
    RUNTIME.block_on(async { create_webrtc_pair().await })
}

static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("tokio runtime"));

fn spawn_task<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(future);
    } else {
        RUNTIME.spawn(future);
    }
}

fn spawn_on_global<F>(future: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    RUNTIME.spawn(future)
}

fn build_api(setting: SettingEngine) -> Result<API, TransportError> {
    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .map_err(to_setup_error)?;

    let mut registry = Registry::new();
    registry =
        register_default_interceptors(registry, &mut media_engine).map_err(to_setup_error)?;

    Ok(APIBuilder::new()
        .with_setting_engine(setting)
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build())
}

async fn attach_vnet_to_router(
    vnet: &Arc<Net>,
    router: &Arc<AsyncMutex<Router>>,
) -> Result<(), TransportError> {
    let nic = vnet.get_nic().map_err(to_setup_error)?;
    {
        let nic_clone = Arc::clone(&nic);
        let mut router_guard = router.lock().await;
        router_guard
            .add_net(nic_clone)
            .await
            .map_err(to_setup_error)?;
    }
    {
        let nic_guard = nic.lock().await;
        nic_guard
            .set_router(Arc::clone(router))
            .await
            .map_err(to_setup_error)?;
    }
    Ok(())
}

struct WebRtcTransport {
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    outbound_seq: AtomicU64,
    outbound_tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
    inbound_rx: Mutex<CrossbeamReceiver<TransportMessage>>,
    _pc: Arc<RTCPeerConnection>,
    _dc: Arc<RTCDataChannel>,
    _router: Option<Arc<AsyncMutex<Router>>>,
    _signaling: Option<Arc<SignalingClient>>,
    encryption: Arc<EncryptionManager>,
}

impl WebRtcTransport {
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: TransportKind,
        id: TransportId,
        peer: TransportId,
        pc: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
        router: Option<Arc<AsyncMutex<Router>>>,
        dc_ready: Option<Arc<Notify>>,
        signaling: Option<Arc<SignalingClient>>,
        handshake_complete: Option<Arc<AtomicBool>>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = crossbeam_unbounded();
        let handler_id = id;
        let tx_clone = inbound_tx.clone();
        tracing::debug!(target = "webrtc", transport_id = ?handler_id, "registering data channel handler");
        let encryption = Arc::new(EncryptionManager::new());
        let encryption_clone_for_handler = Arc::clone(&encryption);
        dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let sender = tx_clone.clone();
            let log_id = handler_id;
            let encryption = Arc::clone(&encryption_clone_for_handler);
            Box::pin(async move {
                let bytes = msg.data.to_vec();
                let payload = if encryption.is_enabled() {
                    match encryption.decrypt(&bytes) {
                        Ok(plaintext) => plaintext,
                        Err(err) => {
                            tracing::warn!(
                                target = "webrtc",
                                transport_id = ?log_id,
                                error = %err,
                                "failed to decrypt inbound frame"
                            );
                            return;
                        }
                    }
                } else {
                    bytes
                };
                if let Some(message) = decode_message(&payload) {
                    tracing::debug!(
                        target = "webrtc",
                        transport_id = ?log_id,
                        frame_len = payload.len(),
                        sequence = message.sequence,
                        "received frame"
                    );
                    if let Err(err) = sender.send(message) {
                        tracing::warn!(
                            target = "webrtc",
                            transport_id = ?log_id,
                            error = %err,
                            "failed to enqueue inbound message"
                        );
                    }
                } else {
                    tracing::warn!(
                        target = "webrtc",
                        transport_id = ?log_id,
                        frame_len = payload.len(),
                        "failed to decode message"
                    );
                }
            })
        }));
        let pc_for_close = pc.clone();
        let handshake_for_close = handshake_complete.clone();
        dc.on_error(Box::new(move |err| {
            let log_id = handler_id;
            Box::pin(async move {
                tracing::warn!(target = "webrtc", transport_id = ?log_id, error = %err, "data channel error");
            })
        }));
        dc.on_close(Box::new(move || {
            let log_id = handler_id;
            let pc_clone = pc_for_close.clone();
            let handshake_for_close = handshake_for_close.clone();
            Box::pin(async move {
                let pc_state = pc_clone.connection_state();
                let ice_state = pc_clone.ice_connection_state();
                if let Some(flag) = handshake_for_close.as_ref() {
                    if !flag.load(Ordering::SeqCst) {
                        tracing::debug!(
                            target = "webrtc",
                            transport_id = ?log_id,
                            pc_state = ?pc_state,
                            ice_state = ?ice_state,
                            "data channel closed before readiness handshake completed"
                        );
                        return;
                    }
                }
                tracing::debug!(
                    target = "webrtc",
                    transport_id = ?log_id,
                    pc_state = ?pc_state,
                    ice_state = ?ice_state,
                    "data channel closed"
                );
            })
        }));

        let (outbound_tx, mut outbound_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let dc_clone = dc.clone();
        let transport_id = id;
        let dc_ready_signal = dc_ready.clone();
        spawn_on_global(async move {
            if let Some(notify) = dc_ready_signal {
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "start"
                );
                notify.notified().await;
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "end"
                );
                tracing::debug!(target = "webrtc", transport_id = ?transport_id, "dc ready triggered");
            } else {
                tracing::debug!(target = "webrtc", transport_id = ?transport_id, "dc ready immediate");
            }
            tracing::debug!(target = "webrtc", transport_id = ?transport_id, "sender loop start");
            loop {
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "outbound_rx.recv",
                    state = "start"
                );
                let maybe_bytes = outbound_rx.recv().await;
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "outbound_rx.recv",
                    state = "end",
                    has_bytes = maybe_bytes.is_some()
                );
                let bytes = match maybe_bytes {
                    Some(bytes) => bytes,
                    None => break,
                };
                tracing::debug!(
                    target = "webrtc",
                    transport_id = ?transport_id,
                    queued_len = bytes.len(),
                    "dequeued outbound"
                );
                let data = Bytes::from(bytes);
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "start"
                );
                let before = dc_clone.buffered_amount().await;
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "end",
                    buffered_before = before
                );
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "start",
                    payload_len = data.len()
                );
                let send_result = timeout(CONNECT_TIMEOUT, dc_clone.send(&data)).await;
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "end",
                    result = ?send_result
                );
                match send_result {
                    Ok(Ok(bytes_written)) => {
                        tracing::debug!(
                            target = "beach_human::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "start"
                        );
                        let after = dc_clone.buffered_amount().await;
                        tracing::debug!(
                            target = "beach_human::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "end",
                            buffered_after = after
                        );
                        tracing::debug!(
                            target = "webrtc",
                            transport_id = ?transport_id,
                            bytes_written,
                            ready_state = ?dc_clone.ready_state(),
                            payload = ?&data[..],
                            buffered_before = before,
                            buffered_after = after,
                            "sent frame"
                        );
                    }
                    Ok(Err(err)) => {
                        let err_display = err.to_string();
                        if err_display.contains("DataChannel is not opened") {
                            tracing::debug!(
                                target = "webrtc",
                                transport_id = ?transport_id,
                                error = %err_display,
                                "dropping outbound frame: data channel not open"
                            );
                        } else {
                            tracing::warn!(
                                target = "webrtc",
                                transport_id = ?transport_id,
                                error = %err_display,
                                "webrtc send error"
                            );
                        }
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            target = "webrtc",
                            transport_id = ?transport_id,
                            "webrtc send timed out"
                        );
                        break;
                    }
                }
            }
            tracing::debug!(target = "webrtc", transport_id = ?transport_id, "sender loop ended");
        });

        Self {
            kind,
            id,
            peer,
            outbound_seq: AtomicU64::new(0),
            outbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            _pc: pc,
            _dc: dc,
            _router: router,
            _signaling: signaling,
            encryption,
        }
    }

    fn enable_encryption(&self, result: &HandshakeResult) -> Result<(), TransportError> {
        self.encryption.enable(result)
    }
}

impl Transport for WebRtcTransport {
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
        let mut bytes = encode_message(&message);
        if self.encryption.is_enabled() {
            bytes = self.encryption.encrypt(&bytes)?;
        }
        tracing::info!(
            target = "webrtc",
            transport_id = ?self.id,
            payload_len = bytes.len(),
            sequence = message.sequence,
            "queueing outbound message"
        );
        self.outbound_tx
            .send(bytes)
            .map_err(|_| TransportError::ChannelClosed)
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
        tracing::debug!(
            target = "webrtc",
            transport_id = ?self.id,
            timeout = ?timeout_duration,
            "waiting for inbound message"
        );
        let receiver = self.inbound_rx.lock().unwrap();
        let result = receiver.recv_timeout(timeout_duration);
        match result {
            Ok(message) => {
                tracing::debug!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    sequence = message.sequence,
                    payload = ?message.payload,
                    "received inbound message"
                );
                Ok(message)
            }
            Err(CrossbeamRecvTimeoutError::Timeout) => {
                tracing::debug!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    "recv timed out"
                );
                Err(TransportError::Timeout)
            }
            Err(CrossbeamRecvTimeoutError::Disconnected) => {
                tracing::warn!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    "recv channel closed"
                );
                Err(TransportError::ChannelClosed)
            }
        }
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        let receiver = self.inbound_rx.lock().unwrap();
        match receiver.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(CrossbeamTryRecvError::Empty) => Ok(None),
            Err(CrossbeamTryRecvError::Disconnected) => Err(TransportError::ChannelClosed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WebRtcRole {
    Offerer,
    Answerer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WebRtcSdpPayload {
    sdp: String,
    #[serde(rename = "type")]
    typ: String,
    handshake_id: String,
    from_peer: String,
    to_peer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<SealedEnvelope>,
}

impl WebRtcSdpPayload {
    fn to_session_description(
        &self,
        passphrase: Option<&str>,
    ) -> Result<RTCSessionDescription, TransportError> {
        session_description_from_payload(self, passphrase)
    }
}

const OFFERER_MAX_NEGOTIATORS: usize = 128;

pub struct OffererAcceptedTransport {
    pub peer_id: String,
    pub handshake_id: String,
    pub metadata: HashMap<String, String>,
    pub connection: WebRtcConnection,
}

pub struct OffererSupervisor {
    inner: Arc<OffererInner>,
    accepted_rx: AsyncMutex<tokio_mpsc::UnboundedReceiver<OffererAcceptedTransport>>,
}

struct OffererInner {
    client: Client,
    signaling_client: Arc<SignalingClient>,
    signaling_base: String,
    poll_interval: Duration,
    passphrase: Option<String>,
    accepted_tx: tokio_mpsc::UnboundedSender<OffererAcceptedTransport>,
    peer_tasks: AsyncMutex<HashMap<String, PeerNegotiatorHandle>>,
    peer_states: AsyncMutex<HashMap<String, PeerLifecycleState>>,
    max_negotiators: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeerLifecycleState {
    Negotiating,
    Established,
}

struct PeerNegotiatorHandle {
    cancel: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl OffererSupervisor {
    pub async fn connect(
        signaling_url: &str,
        poll_interval: Duration,
        passphrase: Option<&str>,
        request_mcp_channel: bool,
    ) -> Result<(Arc<Self>, OffererAcceptedTransport), TransportError> {
        let signaling_client = SignalingClient::connect(
            signaling_url,
            WebRtcRole::Offerer,
            passphrase,
            None,
            request_mcp_channel,
        )
        .await?;
        let client = Client::new();
        let signaling_base = signaling_url.trim_end_matches('/').to_string();
        let (accepted_tx, accepted_rx) = tokio_mpsc::unbounded_channel();

        let inner = Arc::new(OffererInner {
            client,
            signaling_client,
            signaling_base,
            poll_interval,
            passphrase: passphrase.map(|p| p.to_string()),
            accepted_tx,
            peer_tasks: AsyncMutex::new(HashMap::new()),
            peer_states: AsyncMutex::new(HashMap::new()),
            max_negotiators: OFFERER_MAX_NEGOTIATORS,
        });

        let remote_events = inner.signaling_client.remote_events().await?;
        OffererInner::start_event_loop(inner.clone(), remote_events, request_mcp_channel);

        let supervisor = Arc::new(OffererSupervisor {
            inner,
            accepted_rx: AsyncMutex::new(accepted_rx),
        });

        let first = supervisor
            .next()
            .await
            .map_err(|_| TransportError::ChannelClosed)?;

        Ok((supervisor, first))
    }

    pub async fn next(&self) -> Result<OffererAcceptedTransport, TransportError> {
        let mut rx = self.accepted_rx.lock().await;
        rx.recv().await.ok_or(TransportError::ChannelClosed)
    }

    pub fn signaling_client(&self) -> Arc<SignalingClient> {
        Arc::clone(&self.inner.signaling_client)
    }
}

impl OffererInner {
    fn start_event_loop(
        inner: Arc<Self>,
        mut remote_events: tokio_mpsc::UnboundedReceiver<RemotePeerEvent>,
        request_mcp_channel: bool,
    ) {
        tokio::spawn(async move {
            while let Some(event) = remote_events.recv().await {
                match event {
                    RemotePeerEvent::Joined(joined) => {
                        inner.handle_peer_join(joined, request_mcp_channel).await;
                    }
                    RemotePeerEvent::Left(left) => {
                        inner.handle_peer_left(&left.peer_id).await;
                    }
                }
            }

            let mut tasks = inner.peer_tasks.lock().await;
            for (_, handle) in tasks.drain() {
                handle.cancel.store(true, Ordering::SeqCst);
                handle.task.abort();
            }
        });
    }

    async fn handle_peer_join(
        self: &Arc<Self>,
        joined: RemotePeerJoined,
        request_mcp_channel: bool,
    ) {
        if joined.peer.role != PeerRole::Client {
            tracing::debug!(
                target = "webrtc",
                peer_id = %joined.peer.id,
                role = ?joined.peer.role,
                "ignoring peer join for non-client role"
            );
            return;
        }

        {
            let mut states = self.peer_states.lock().await;
            if let Some(state) = states.get(&joined.peer.id) {
                match state {
                    PeerLifecycleState::Negotiating => {
                        tracing::trace!(
                            target = "webrtc",
                            peer_id = %joined.peer.id,
                            "peer negotiator already active; ignoring new join"
                        );
                        tracing::debug!(
                            target = "webrtc",
                            peer_id = %joined.peer.id,
                            "peer negotiator already active"
                        );
                        return;
                    }
                    PeerLifecycleState::Established => {
                        tracing::trace!(
                            target = "webrtc",
                            peer_id = %joined.peer.id,
                            "peer already has established transport; ignoring join"
                        );
                        tracing::debug!(
                            target = "webrtc",
                            peer_id = %joined.peer.id,
                            "peer already has established transport"
                        );
                        return;
                    }
                }
            }
            states.insert(joined.peer.id.clone(), PeerLifecycleState::Negotiating);
            tracing::trace!(
                target = "webrtc",
                peer_id = %joined.peer.id,
                "peer lifecycle transitioned to negotiating"
            );
        }

        let mut tasks = self.peer_tasks.lock().await;
        if tasks.contains_key(&joined.peer.id) {
            tracing::debug!(
                target = "webrtc",
                peer_id = %joined.peer.id,
                "peer negotiator already active"
            );
            return;
        }
        if tasks.len() >= self.max_negotiators {
            tracing::warn!(
                target = "webrtc",
                peer_id = %joined.peer.id,
                active = tasks.len(),
                max = self.max_negotiators,
                "dropping peer join due to negotiator capacity"
            );
            return;
        }

        tracing::info!(
            target = "webrtc",
            peer_id = %joined.peer.id,
            "registering peer negotiator"
        );
        tracing::debug!(
            target = "webrtc",
            peer_id = %joined.peer.id,
            active_tasks = tasks.len(),
            "spawning negotiator task for joined peer"
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let peer_id = joined.peer.id.clone();
        let inner_for_task = Arc::clone(self);
        let cancel_for_task = cancel.clone();
        let peer_id_for_task = peer_id.clone();
        let mcp_flag = request_mcp_channel;
        let task = tokio::spawn(async move {
            let result = negotiate_offerer_peer(
                inner_for_task.clone(),
                joined,
                cancel_for_task.clone(),
                mcp_flag,
            )
            .await;
            inner_for_task
                .finalize_peer(&peer_id_for_task, result, cancel_for_task)
                .await;
        });

        let previous = tasks.insert(peer_id.clone(), PeerNegotiatorHandle { cancel, task });
        tracing::trace!(
            target = "webrtc",
            peer_id = %peer_id,
            active_tasks = tasks.len(),
            replaced_existing = previous.is_some(),
            "peer negotiator registered"
        );
        drop(tasks);
        // state already set to Negotiating above
    }

    async fn handle_peer_left(self: &Arc<Self>, peer_id: &str) {
        tracing::info!(
            target = "webrtc",
            peer_id = %peer_id,
            "handling peer left event"
        );
        let mut tasks = self.peer_tasks.lock().await;
        if let Some(handle) = tasks.remove(peer_id) {
            tracing::debug!(
                target = "webrtc",
                peer_id = %peer_id,
                "setting cancel flag and aborting negotiator task for departed peer"
            );
            handle.cancel.store(true, Ordering::SeqCst);
            handle.task.abort();
        } else {
            tracing::debug!(
                target = "webrtc",
                peer_id = %peer_id,
                "no active negotiator task found for departed peer"
            );
        }

        let mut states = self.peer_states.lock().await;
        states.remove(peer_id);
    }

    async fn finalize_peer(
        self: Arc<Self>,
        peer_id: &str,
        result: Result<Option<OffererAcceptedTransport>, TransportError>,
        cancel_flag: Arc<AtomicBool>,
    ) {
        {
            let mut tasks = self.peer_tasks.lock().await;
            tasks.remove(peer_id);
        }

        match result {
            Ok(Some(accepted)) => {
                {
                    let mut states = self.peer_states.lock().await;
                    if let Some(entry) = states.get_mut(peer_id) {
                        *entry = PeerLifecycleState::Established;
                    } else {
                        states.insert(peer_id.to_string(), PeerLifecycleState::Established);
                    }
                    tracing::trace!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        state = ?states.get(peer_id),
                        "peer lifecycle updated to established"
                    );
                }
                if self.accepted_tx.send(accepted).is_err() {
                    tracing::debug!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        "dropping accepted transport because receiver closed"
                    );
                }
            }
            Ok(None) => {
                {
                    let mut states = self.peer_states.lock().await;
                    states.remove(peer_id);
                    tracing::trace!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        "peer lifecycle entry removed after negotiator concluded without transport"
                    );
                }
                tracing::debug!(
                    target = "webrtc",
                    peer_id = %peer_id,
                    cancelled = cancel_flag.load(Ordering::SeqCst),
                    "peer negotiation concluded without establishing transport"
                );
            }
            Err(err) => {
                {
                    let mut states = self.peer_states.lock().await;
                    states.remove(peer_id);
                    tracing::trace!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        "peer lifecycle entry removed after negotiator error"
                    );
                }
                tracing::warn!(
                    target = "webrtc",
                    peer_id = %peer_id,
                    cancelled = cancel_flag.load(Ordering::SeqCst),
                    error = %err,
                    "peer negotiation ended with error"
                );
            }
        }
    }
}

async fn negotiate_offerer_peer(
    inner: Arc<OffererInner>,
    joined: RemotePeerJoined,
    cancel_flag: Arc<AtomicBool>,
    request_mcp_channel: bool,
) -> Result<Option<OffererAcceptedTransport>, TransportError> {
    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let RemotePeerJoined { peer, signals, .. } = joined;

    let mut setting = SettingEngine::default();
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    let mut config = RTCConfiguration::default();
    if std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_err() {
        config.ice_servers = vec![webrtc::ice_transport::ice_server::RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }];
    }

    let pc = Arc::new(
        api.new_peer_connection(config)
            .await
            .map_err(to_setup_error)?,
    );
    let channels = WebRtcChannels::new();
    let pending_ice = Arc::new(AsyncMutex::new(Vec::new()));
    let handshake_id = Uuid::new_v4().to_string();
    let handshake_id_arc = Arc::new(handshake_id.clone());
    let peer_id = peer.id.clone();
    let peer_id_for_candidates = peer_id.clone();

    let signaling_for_candidates = Arc::clone(&inner.signaling_client);
    let cancel_for_candidates = Arc::clone(&cancel_flag);
    let handshake_for_candidates = handshake_id_arc.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        let peer_id = peer_id_for_candidates.clone();
        let cancel_flag = Arc::clone(&cancel_for_candidates);
        let handshake_id = handshake_for_candidates.clone();
        Box::pin(async move {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            if let Some(cand) = candidate {
                if let Err(err) = signaling
                    .send_ice_candidate_to_peer(cand, handshake_id.as_str(), &peer_id)
                    .await
                {
                    tracing::warn!(
                        target = "webrtc",
                        peer_id = %peer_id,
                        error = %err,
                        "failed to send local ice candidate"
                    );
                }
            }
        })
    }));

    let signaling_for_incoming = Arc::clone(&inner.signaling_client);
    let pc_for_incoming = Arc::clone(&pc);
    let pending_for_incoming = Arc::clone(&pending_ice);
    let handshake_for_incoming = handshake_id_arc.clone();
    let cancel_for_incoming = Arc::clone(&cancel_flag);
    let mut signal_stream = signals;
    let peer_id_for_incoming = peer_id.clone();
    let passphrase_for_signals = inner.passphrase.clone();
    let ice_task = spawn_on_global(async move {
        while let Some(signal) = signal_stream.recv().await {
            if cancel_for_incoming.load(Ordering::SeqCst) {
                break;
            }
            if let WebRTCSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                handshake_id,
                sealed,
            } = signal
            {
                if handshake_id != handshake_for_incoming.as_str() {
                    tracing::debug!(
                        target = "webrtc",
                        peer_id = %peer_id_for_incoming,
                        handshake_id = %handshake_id,
                        "ignoring remote ice candidate for stale handshake"
                    );
                    continue;
                }
                let local_peer_id = signaling_for_incoming
                    .assigned_peer_id()
                    .await
                    .unwrap_or_else(|| signaling_for_incoming.peer_id().to_string());
                let resolved = match resolve_ice_candidate(
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                    sealed,
                    passphrase_for_signals.as_deref(),
                    handshake_for_incoming.as_str(),
                    &peer_id_for_incoming,
                    local_peer_id.as_str(),
                ) {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        tracing::warn!(
                            target = "webrtc",
                            peer_id = %peer_id_for_incoming,
                            error = %err,
                            "failed to decode remote ice candidate"
                        );
                        continue;
                    }
                };
                let init = RTCIceCandidateInit {
                    candidate: resolved.candidate,
                    sdp_mid: resolved.sdp_mid,
                    sdp_mline_index: resolved.sdp_mline_index.map(|idx| idx as u16),
                    username_fragment: None,
                };
                let has_remote = pc_for_incoming.remote_description().await.is_some();
                if !has_remote {
                    let mut queue = pending_for_incoming.lock().await;
                    queue.push(init);
                    continue;
                }
                if let Err(err) = pc_for_incoming.add_ice_candidate(init.clone()).await {
                    tracing::warn!(
                        target = "webrtc",
                        peer_id = %peer_id_for_incoming,
                        error = %err,
                        "failed to add remote ice candidate"
                    );
                    let mut queue = pending_for_incoming.lock().await;
                    queue.push(init);
                }
            }
        }
    });

    let dc_notify = Arc::new(Notify::new());
    let dc_open_notify = dc_notify.clone();
    let dc_init = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };
    let dc = pc
        .create_data_channel("beach-human", Some(dc_init))
        .await
        .map_err(to_setup_error)?;
    dc.on_open(Box::new(move || {
        let notify = dc_open_notify.clone();
        Box::pin(async move {
            notify.notify_waiters();
            notify.notify_one();
        })
    }));

    let secure_transport_active = secure_transport_enabled()
        && inner
            .passphrase
            .as_ref()
            .map(|p| !p.trim().is_empty())
            .unwrap_or(false);
    tracing::info!(
        target = "webrtc",
        role = "offerer",
        secure_transport_active,
        has_passphrase = inner.passphrase.is_some(),
        env_enabled = secure_transport_enabled(),
        "offerer: checking if secure transport should be enabled"
    );
    let handshake_dc = if secure_transport_active {
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            label = HANDSHAKE_CHANNEL_LABEL,
            "offerer: creating handshake data channel"
        );
        Some(
            pc.create_data_channel(HANDSHAKE_CHANNEL_LABEL, Some(handshake_channel_init()))
                .await
                .map_err(to_setup_error)?,
        )
    } else {
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            "offerer: NOT creating handshake channel (secure transport disabled)"
        );
        None
    };
    if let Some(ref dc) = handshake_dc {
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            channel_state = ?dc.ready_state(),
            "offerer: handshake channel created"
        );
    }

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = pc.close().await;
        ice_task.abort();
        return Ok(None);
    }

    pc.set_local_description(pc.create_offer(None).await.map_err(to_setup_error)?)
        .await
        .map_err(to_setup_error)?;
    wait_for_local_description(&pc).await?;

    let local_desc = pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing local description".into()))?;
    let offerer_peer_id = inner
        .signaling_client
        .assigned_peer_id()
        .await
        .unwrap_or_else(|| inner.signaling_client.peer_id().to_string());

    let payload = payload_from_description(
        &local_desc,
        &handshake_id,
        &offerer_peer_id,
        &peer.id,
        inner.passphrase.as_deref(),
    )?;

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = pc.close().await;
        ice_task.abort();
        return Ok(None);
    }

    post_sdp(&inner.client, &inner.signaling_base, "offer", &[], &payload).await?;

    let answer = poll_answer_for_peer(
        &inner.client,
        &inner.signaling_base,
        inner.poll_interval,
        &handshake_id,
    )
    .await?;

    let remote_desc = answer.to_session_description(inner.passphrase.as_deref())?;
    pc.set_remote_description(remote_desc)
        .await
        .map_err(to_setup_error)?;

    {
        let mut queued = pending_ice.lock().await;
        for candidate in queued.drain(..) {
            if let Err(err) = pc.add_ice_candidate(candidate.clone()).await {
                tracing::warn!(
                    target = "webrtc",
                    peer_id = %peer.id,
                    error = %err,
                    "failed to add buffered remote ice candidate"
                );
            }
        }
    }

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = pc.close().await;
        ice_task.abort();
        return Ok(None);
    }

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        "waiting for datachannel to open (15s timeout)"
    );
    if tokio::time::timeout(Duration::from_secs(15), dc_notify.notified())
        .await
        .is_err()
    {
        tracing::warn!(
            target = "webrtc",
            peer_id = %peer.id,
            "datachannel open timeout, closing peer connection"
        );
        let _ = pc.close().await;
        ice_task.abort();
        return Err(TransportError::Setup(
            "offerer data channel did not open".into(),
        ));
    }

    let local_id = next_transport_id();
    let remote_id = next_transport_id();
    let handshake_complete = Arc::new(AtomicBool::new(false));
    let dc_state_before = dc.ready_state();
    let pc_state_before = pc.connection_state();
    let transport = Arc::new(WebRtcTransport::new(
        TransportKind::WebRtc,
        local_id,
        remote_id,
        pc.clone(),
        dc,
        None,
        Some(dc_notify.clone()),
        Some(Arc::clone(&inner.signaling_client)),
        Some(Arc::clone(&handshake_complete)),
    ));
    let transport_dyn: Arc<dyn Transport> = transport.clone();
    channels.publish("beach-human".to_string(), transport_dyn.clone());

    // Run secure handshake BEFORE waiting for __ready__ sentinel
    // The answerer will send __ready__ only after completing the handshake,
    // so the offerer must initiate the handshake first
    tracing::info!(
        target = "webrtc",
        role = "offerer",
        peer_id = %peer.id,
        secure_transport_active,
        has_passphrase = inner.passphrase.is_some(),
        has_handshake_channel = handshake_dc.is_some(),
        "offerer: evaluating whether to run secure handshake"
    );
    let secure_context = if let (true, Some(passphrase), Some(handshake_channel)) = (
        secure_transport_active,
        inner.passphrase.as_ref(),
        handshake_dc.clone(),
    ) {
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            handshake_id = %handshake_id,
            offerer_peer = %offerer_peer_id,
            remote_peer = %peer_id,
            channel_state = ?handshake_channel.ready_state(),
            "offerer: about to run handshake as Initiator"
        );
        let prologue_context = build_prologue_context(&handshake_id, &offerer_peer_id, &peer_id);
        let params = HandshakeParams {
            passphrase: passphrase.clone(),
            handshake_id: handshake_id.clone(),
            local_peer_id: offerer_peer_id.clone(),
            remote_peer_id: peer_id.clone(),
            prologue_context,
        };
        tracing::debug!(
            target = "webrtc",
            role = "offerer",
            handshake_id = %handshake_id,
            "offerer: calling run_handshake as Initiator"
        );
        let result = run_handshake(HandshakeRole::Initiator, handshake_channel, params).await?;
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            handshake_id = %handshake_id,
            verification = %result.verification_code,
            "offerer: handshake completed successfully as Initiator"
        );
        transport.enable_encryption(&result)?;
        Some(Arc::new(result))
    } else {
        tracing::info!(
            target = "webrtc",
            role = "offerer",
            peer_id = %peer.id,
            secure_transport_active,
            has_passphrase = inner.passphrase.is_some(),
            has_handshake_channel = handshake_dc.is_some(),
            "offerer: skipping secure handshake (conditions not met)"
        );
        None
    };

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        transport_id = ?local_id,
        dc_state = ?dc_state_before,
        pc_state = ?pc_state_before,
        "starting readiness handshake"
    );

    // Give the data channel on_message callback a chance to fire and enqueue
    // any messages that arrived immediately when the channel opened
    tracing::trace!(
        target = "webrtc",
        peer_id = %peer.id,
        "sleeping 10ms before polling for __ready__"
    );
    sleep(Duration::from_millis(10)).await;
    let dc_state_after = transport._dc.ready_state();
    let pc_state_after = pc.connection_state();
    tracing::trace!(
        target = "webrtc",
        peer_id = %peer.id,
        dc_state_after_sleep = ?dc_state_after,
        pc_state_after_sleep = ?pc_state_after,
        "sleep completed"
    );

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        "beginning to poll for __ready__ sentinel"
    );

    let mut ready_seen = false;
    let mut readiness_attempts = 0usize;
    for attempt in 0..READY_ACK_POLL_ATTEMPTS {
        readiness_attempts = attempt + 1;
        if cancel_flag.load(Ordering::SeqCst) {
            tracing::warn!(
                target = "webrtc",
                peer_id = %peer.id,
                attempt = attempt,
                "readiness handshake cancelled via cancel_flag, closing peer connection"
            );
            let _ = pc.close().await;
            ice_task.abort();
            return Ok(None);
        }
        match transport.try_recv() {
            Ok(Some(message)) => {
                tracing::debug!(
                    target = "webrtc",
                    peer_id = %peer.id,
                    attempt = attempt,
                    payload = ?message.payload,
                    payload_text = ?message.payload.as_text(),
                    "offerer received message during readiness handshake"
                );
                if message.payload.as_text() == Some("__ready__") {
                    ready_seen = true;
                    tracing::info!(
                        target = "webrtc",
                        peer_id = %peer.id,
                        attempt = attempt,
                        "received __ready__ sentinel from answerer"
                    );
                    break;
                } else {
                    tracing::debug!(
                        target = "webrtc",
                        peer_id = %peer.id,
                        payload = ?message.payload,
                        "received unexpected message during readiness handshake"
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(
                    target = "webrtc",
                    peer_id = %peer.id,
                    error = %err,
                    "failed polling for readiness ack"
                );
                break;
            }
        }
        if attempt + 1 < READY_ACK_POLL_ATTEMPTS {
            sleep(READY_ACK_POLL_INTERVAL).await;
        }
    }

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        attempts = readiness_attempts,
        ready_seen = ready_seen,
        "readiness handshake polling finished"
    );

    if !ready_seen {
        tracing::warn!(
            target = "webrtc",
            peer_id = %peer.id,
            transport_id = ?local_id,
            "closing peer connection: did not receive __ready__ sentinel"
        );
        let _ = pc.close().await;
        ice_task.abort();
        return Err(TransportError::Setup(
            "offerer missing data channel readiness sentinel".into(),
        ));
    }

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        "sending __offer_ready__ sentinel to answerer"
    );

    tracing::debug!(
        target = "webrtc",
        peer_id = %peer.id,
        handshake_id = %handshake_id,
        attempts = readiness_attempts,
        "readiness handshake completed"
    );
    handshake_complete.store(true, Ordering::SeqCst);

    if let Err(err) = transport.send_text("__offer_ready__") {
        tracing::warn!(
            target = "webrtc",
            peer_id = %peer.id,
            error = %err,
            "failed to send offer ready sentinel"
        );
    }

    tracing::info!(
        target = "webrtc",
        peer_id = %peer.id,
        handshake_id = %handshake_id,
        "offerer transport established"
    );

    if request_mcp_channel {
        let mcp_init = RTCDataChannelInit {
            ordered: Some(true),
            ..Default::default()
        };
        let mcp_dc = pc
            .create_data_channel(MCP_CHANNEL_LABEL, Some(mcp_init))
            .await
            .map_err(to_setup_error)?;
        let mcp_transport = Arc::new(WebRtcTransport::new(
            TransportKind::WebRtc,
            next_transport_id(),
            next_transport_id(),
            pc.clone(),
            mcp_dc,
            None,
            None,
            Some(Arc::clone(&inner.signaling_client)),
            Some(Arc::clone(&handshake_complete)),
        ));
        let mcp_transport_dyn: Arc<dyn Transport> = mcp_transport;
        channels.publish(MCP_CHANNEL_LABEL.to_string(), mcp_transport_dyn);
        tracing::info!(
            target = "webrtc",
            peer_id = %peer.id,
            handshake_id = %handshake_id,
            "offerer published mcp data channel"
        );
    }

    let mut peer_metadata = peer.metadata.clone().unwrap_or_default();
    if let Some(ref result) = secure_context {
        peer_metadata
            .entry("secure_verification".to_string())
            .or_insert(result.verification_code.clone());
    }

    Ok(Some(OffererAcceptedTransport {
        peer_id: peer_id,
        handshake_id,
        metadata: peer_metadata,
        connection: WebRtcConnection::new(transport_dyn, channels, secure_context),
    }))
}

pub async fn connect_via_signaling(
    signaling_url: &str,
    role: WebRtcRole,
    poll_interval: Duration,
    passphrase: Option<&str>,
    label: Option<&str>,
    request_mcp_channel: bool,
) -> Result<WebRtcConnection, TransportError> {
    match role {
        WebRtcRole::Offerer => {
            let (_supervisor, accepted) = OffererSupervisor::connect(
                signaling_url,
                poll_interval,
                passphrase,
                request_mcp_channel,
            )
            .await?;
            Ok(accepted.connection)
        }
        WebRtcRole::Answerer => {
            connect_answerer(signaling_url, poll_interval, passphrase, label).await
        }
    }
}

async fn connect_answerer(
    signaling_url: &str,
    poll_interval: Duration,
    passphrase: Option<&str>,
    label: Option<&str>,
) -> Result<WebRtcConnection, TransportError> {
    let passphrase_owned = passphrase.map(|s| s.to_string());
    let secure_transport_active = secure_transport_enabled()
        && passphrase_owned
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    let (secure_tx, secure_rx) =
        oneshot::channel::<Result<Option<Arc<HandshakeResult>>, TransportError>>();
    let secure_sender = Arc::new(AsyncMutex::new(Some(secure_tx)));
    if !secure_transport_active {
        if let Some(sender) = secure_sender.lock().await.take() {
            let _ = sender.send(Ok(None));
        }
    }
    let client = Client::new();
    let signaling_client = SignalingClient::connect(
        signaling_url,
        WebRtcRole::Answerer,
        passphrase,
        label.map(|s| s.to_string()),
        false,
    )
    .await?;
    let (expected_remote_peer, _) = signaling_client
        .wait_for_remote_peer_with_generation()
        .await?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        expected_remote = %expected_remote_peer,
        "initialized expected remote peer"
    );
    signaling_client
        .lock_remote_peer(&expected_remote_peer)
        .await;
    let assigned_peer_id = signaling_client
        .assigned_peer_id()
        .await
        .expect("assigned peer id");

    let offer_payload = loop {
        tracing::debug!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "start"
        );
        let peer_param = [("peer_id", assigned_peer_id.as_str())];
        let fetch_attempt = fetch_sdp(&client, signaling_url, "offer", &peer_param).await;
        tracing::debug!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "end",
            result = ?fetch_attempt
        );
        match fetch_attempt? {
            Some(payload) => {
                if payload.from_peer != expected_remote_peer {
                    tracing::warn!(
                        target = "beach_human::transport::webrtc",
                        role = "answerer",
                        expected_remote = %expected_remote_peer,
                        received_remote = %payload.from_peer,
                        "ignoring offer from unexpected peer"
                    );
                    continue;
                }
                tracing::info!(
                    target = "beach_human::transport::webrtc",
                    role = "answerer",
                    handshake_id = %payload.handshake_id,
                    remote_peer = %payload.from_peer,
                    "accepted offer"
                );
                break payload;
            }
            None => {
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "start",
                    poll_ms = poll_interval.as_millis() as u64
                );
                sleep(poll_interval).await;
                tracing::debug!(
                    target = "beach_human::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "end"
                );
            }
        }
    };
    let offer_desc = session_description_from_payload(&offer_payload, passphrase_owned.as_deref())?;
    let handshake_id = Arc::new(offer_payload.handshake_id.clone());
    let remote_offer_peer = offer_payload.from_peer.clone();

    let mut setting = SettingEngine::default();
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    // Add a public STUN server so we gather server-reflexive candidates.
    // Without this, host-only candidates can fail in common NAT setups where the
    // browser uses mDNS/srflx and the offerer has no reflexive candidates.
    let mut config = RTCConfiguration::default();
    if std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_err() {
        config.ice_servers = vec![webrtc::ice_transport::ice_server::RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }];
    }

    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "start"
    );
    let pc_result = api.new_peer_connection(config).await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "end",
        result = ?pc_result
    );
    let pc = Arc::new(pc_result.map_err(to_setup_error)?);
    let channels = WebRtcChannels::new();

    let signaling_for_candidates = Arc::clone(&signaling_client);
    let handshake_for_candidates = Arc::clone(&handshake_id);
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        let handshake_id = Arc::clone(&handshake_for_candidates);
        Box::pin(async move {
            if let Some(cand) = candidate {
                tracing::debug!(
                    target = "webrtc",
                    role = "offerer",
                    candidate = %cand.to_string(),
                    "local ice candidate gathered"
                );
                if let Err(err) = signaling
                    .send_ice_candidate(cand, handshake_id.as_str())
                    .await
                {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "answerer candidate send error"
                    );
                }
            }
        })
    }));

    let signaling_for_incoming = Arc::clone(&signaling_client);
    let pc_for_incoming = pc.clone();
    let handshake_for_incoming = Arc::clone(&handshake_id);
    let pending_for_incoming: Arc<AsyncMutex<Vec<RTCIceCandidateInit>>> =
        Arc::new(AsyncMutex::new(Vec::new()));
    let pending_for_incoming_clone = Arc::clone(&pending_for_incoming);
    let remote_offer_peer_for_signals = remote_offer_peer.clone();
    let assigned_peer_id_for_signals = assigned_peer_id.clone();
    let passphrase_for_signals = passphrase_owned.clone();
    spawn_on_global(async move {
        while let Some(signal) = signaling_for_incoming.recv_webrtc_signal().await {
            if let WebRTCSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                handshake_id,
                sealed,
            } = signal
            {
                if handshake_for_incoming.as_str() != handshake_id {
                    tracing::debug!(
                        target = "webrtc",
                        handshake_id,
                        "answerer ignoring remote ICE candidate for stale handshake"
                    );
                    continue;
                }
                let resolved = match resolve_ice_candidate(
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                    sealed,
                    passphrase_for_signals.as_deref(),
                    handshake_for_incoming.as_str(),
                    &remote_offer_peer_for_signals,
                    assigned_peer_id_for_signals.as_str(),
                ) {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        tracing::warn!(
                            target = "webrtc",
                            handshake_id,
                            error = %err,
                            "failed to decode remote ice candidate"
                        );
                        continue;
                    }
                };
                let init = RTCIceCandidateInit {
                    candidate: resolved.candidate,
                    sdp_mid: resolved.sdp_mid,
                    sdp_mline_index: resolved.sdp_mline_index.map(|idx| idx as u16),
                    username_fragment: None,
                };
                let has_remote = pc_for_incoming.remote_description().await.is_some();
                if !has_remote {
                    let mut queue = pending_for_incoming.lock().await;
                    queue.push(init);
                    tracing::debug!(
                        target = "webrtc",
                        role = "answerer",
                        "queued remote ice candidate (remote description not set yet)"
                    );
                    continue;
                }
                if let Err(err) = pc_for_incoming.add_ice_candidate(init.clone()).await {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "answerer failed to add remote ice candidate"
                    );
                    let mut queue = pending_for_incoming.lock().await;
                    queue.push(init);
                }
            }
        }
    });

    let dc_open_notify = Arc::new(Notify::new());
    let transport_slot: Arc<AsyncMutex<Option<Arc<WebRtcTransport>>>> =
        Arc::new(AsyncMutex::new(None));
    let pc_for_dc = pc.clone();
    let notify_clone = dc_open_notify.clone();
    let slot_clone = transport_slot.clone();
    let client_id = next_transport_id();
    let peer_id = next_transport_id();
    tracing::debug!(
        target = "webrtc",
        ?client_id,
        ?peer_id,
        "answerer allocating transport ids"
    );
    let signaling_for_dc = Arc::clone(&signaling_client);
    let channels_registry = channels.clone();
    let secure_sender_holder = Arc::clone(&secure_sender);
    let passphrase_for_secure = passphrase_owned.clone();
    let assigned_peer_for_secure = assigned_peer_id.clone();
    let remote_peer_for_secure = remote_offer_peer.clone();
    let handshake_for_secure = Arc::clone(&handshake_id);
    pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let pc = pc_for_dc.clone();
        let notify = notify_clone.clone();
        let slot = slot_clone.clone();
        let signaling_for_transport = Arc::clone(&signaling_for_dc);
        let channels = channels_registry.clone();
        let secure_sender = Arc::clone(&secure_sender_holder);
        let passphrase_value = passphrase_for_secure.clone();
        let assigned_peer_value = assigned_peer_for_secure.clone();
        let remote_peer_value = remote_peer_for_secure.clone();
        let handshake_value = Arc::clone(&handshake_for_secure);
        Box::pin(async move {
            let label = dc.label().to_string();
            tracing::debug!(
                target = "webrtc",
                role = "answerer",
                label = %label,
                "incoming data channel announced"
            );

            if label == HANDSHAKE_CHANNEL_LABEL {
                if !secure_transport_active {
                    if let Some(sender) = secure_sender.lock().await.take() {
                        let _ = sender.send(Ok(None));
                    }
                    return;
                }
                let Some(passphrase) = passphrase_value.clone() else {
                    if let Some(sender) = secure_sender.lock().await.take() {
                        let _ = sender.send(Ok(None));
                    }
                    return;
                };
                let dc_for_handshake = Arc::clone(&dc);
                let channel_state = dc_for_handshake.ready_state();
                let sender_holder = Arc::clone(&secure_sender);
                let local_peer = assigned_peer_value.clone();
                let remote_peer = remote_peer_value.clone();
                let handshake_id_value = (*handshake_value).clone();
                tracing::info!(
                    target = "webrtc",
                    label = HANDSHAKE_CHANNEL_LABEL,
                    ?channel_state,
                    handshake_id = %handshake_id_value,
                    local_peer = %local_peer,
                    remote_peer = %remote_peer,
                    "spawning handshake task for answerer"
                );
                spawn_on_global(async move {
                    tracing::info!(
                        target = "webrtc",
                        handshake_id = %handshake_id_value,
                        local_peer = %local_peer,
                        remote_peer = %remote_peer,
                        "handshake task started on answerer (inside spawned task)"
                    );
                    let prologue_context = build_prologue_context(
                        &handshake_id_value,
                        local_peer.as_str(),
                        remote_peer.as_str(),
                    );
                    let params = HandshakeParams {
                        passphrase,
                        handshake_id: handshake_id_value.clone(),
                        local_peer_id: local_peer.clone(),
                        remote_peer_id: remote_peer.clone(),
                        prologue_context,
                    };
                    tracing::debug!(
                        target = "webrtc",
                        handshake_id = %handshake_id_value,
                        "calling run_handshake as Responder"
                    );
                    let outcome = run_handshake(HandshakeRole::Responder, dc_for_handshake, params)
                        .await
                        .map(|res| Some(Arc::new(res)));
                    tracing::info!(
                        target = "webrtc",
                        handshake_id = %handshake_id_value,
                        outcome = ?outcome.as_ref().map(|_| "success").unwrap_or("error"),
                        "handshake task completed, sending result"
                    );
                    if let Some(sender) = sender_holder.lock().await.take() {
                        let _ = sender.send(outcome);
                    }
                });
                return;
            }

            let notify_for_open = notify.clone();
            dc.on_open(Box::new(move || {
                let notify = notify_for_open.clone();
                Box::pin(async move {
                    tracing::debug!(target = "webrtc", "data channel opened (answerer)");
                    tracing::debug!(target = "webrtc", "answerer data channel open");
                    notify.notify_waiters();
                    notify.notify_one();
                })
            }));

            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "slot.lock",
                state = "start"
            );
            let mut slot_guard = slot.lock().await;
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "slot.lock",
                state = "end",
                is_populated = slot_guard.is_some()
            );
            let transport = Arc::new(WebRtcTransport::new(
                TransportKind::WebRtc,
                client_id,
                peer_id,
                pc.clone(),
                dc,
                None,
                Some(notify.clone()),
                Some(signaling_for_transport),
                None,
            ));
            let transport_dyn: Arc<dyn Transport> = transport.clone();

            if slot_guard.is_none() {
                slot_guard.replace(transport.clone());
                drop(slot_guard);

                if !secure_transport_active {
                    tracing::debug!(
                        target = "webrtc",
                        role = "answerer",
                        "sending __ready__ sentinel to offerer"
                    );

                    if let Err(err) = transport_dyn.send_text("__ready__") {
                        tracing::warn!(
                            target = "webrtc",
                            error = %err,
                            "answerer readiness ack failed"
                        );
                    } else {
                        tracing::info!(
                            target = "webrtc",
                            role = "answerer",
                            "sent __ready__ sentinel successfully"
                        );
                    }
                }

                channels.publish(label.clone(), transport_dyn.clone());
                return;
            }

            drop(slot_guard);
            channels.publish(label, transport_dyn);
        })
    }));

    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "start"
    );
    pc.set_remote_description(offer_desc)
        .await
        .map_err(to_setup_error)?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "end"
    );

    // Process queued ICE candidates now that remote description is set
    let pending_candidates = {
        let mut queue = pending_for_incoming_clone.lock().await;
        let candidates = queue.drain(..).collect::<Vec<_>>();
        candidates
    };
    if !pending_candidates.is_empty() {
        tracing::debug!(
            target = "webrtc",
            role = "answerer",
            count = pending_candidates.len(),
            "processing queued remote ice candidates"
        );
        for init in pending_candidates {
            if let Err(err) = pc.add_ice_candidate(init).await {
                tracing::warn!(
                    target = "webrtc",
                    role = "answerer",
                    error = %err,
                    "failed to add queued remote ice candidate"
                );
            }
        }
    }

    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.create_answer",
        state = "start"
    );
    let answer_result = pc.create_answer(None).await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.create_answer",
        state = "end",
        result = ?answer_result
    );
    let answer = answer_result.map_err(to_setup_error)?;
    let answer_payload = payload_from_description(
        &answer,
        handshake_id.as_str(),
        assigned_peer_id.as_str(),
        &remote_offer_peer,
        passphrase_owned.as_deref(),
    )?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "start"
    );
    pc.set_local_description(answer)
        .await
        .map_err(to_setup_error)?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "end"
    );
    wait_for_local_description(&pc).await?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "start"
    );
    let post_result = post_sdp(&client, signaling_url, "answer", &[], &answer_payload).await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "end",
        result = ?post_result
    );
    post_result?;
    signaling_client
        .unlock_remote_peer(&expected_remote_peer)
        .await;

    // Verbose connection state tracing for diagnosis
    {
        let _pc_trace = pc.clone();
        pc.on_ice_connection_state_change(Box::new(move |state| {
            let st = state;
            Box::pin(async move {
                tracing::debug!(target = "webrtc", role = "answerer", ice_connection_state = ?st, "peer connection ice state change");
            })
        }));
    }
    {
        let _pc_trace = pc.clone();
        pc.on_signaling_state_change(Box::new(move |state| {
            let st = state;
            Box::pin(async move {
                tracing::debug!(target = "webrtc", role = "answerer", signaling_state = ?st, "peer connection signaling state change");
            })
        }));
    }
    {
        let _pc_trace = pc.clone();
        pc.on_ice_gathering_state_change(Box::new(move |state| {
            let st = state;
            Box::pin(async move {
                tracing::debug!(target = "webrtc", role = "answerer", ice_gathering_state = ?st, "peer connection ice gathering change");
            })
        }));
    }

    let mut attempts: usize = 0;
    let max_attempts = 1000; // 10 seconds timeout (1000 * 10ms)
    let transport = loop {
        attempts = attempts.saturating_add(1);
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "transport_slot.lock",
                state = "start",
                attempts
            );
        }
        let mut transport_guard = transport_slot.lock().await;
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "transport_slot.lock",
                state = "end",
                has_transport = transport_guard.is_some(),
                attempts
            );
        }
        if let Some(transport) = transport_guard.as_ref().cloned() {
            drop(transport_guard);
            break transport;
        }
        if attempts >= max_attempts {
            drop(transport_guard);
            tracing::warn!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                attempts,
                "timeout waiting for data channel to be announced"
            );
            return Err(TransportError::Setup(
                "timeout waiting for data channel".into(),
            ));
        }
        transport_guard.take();
        drop(transport_guard);
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "sleep.retry",
                state = "start",
                poll_ms = 10_u64,
                attempts
            );
        }
        sleep(Duration::from_millis(10)).await;
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "sleep.retry",
                state = "end",
                attempts
            );
        }
    };
    tracing::debug!(target = "webrtc", ?client_id, "answerer transport ready");

    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "start"
    );
    let wait_result = wait_for_connection(&pc).await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "end",
        result = ?wait_result
    );
    wait_result?;

    // Check if transport is already populated (data channel already opened)
    let already_ready = {
        let guard = transport_slot.lock().await;
        guard.is_some()
    };

    if !already_ready {
        tracing::debug!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "dc_open_notify.timeout",
            state = "start"
        );
        let notify_result = timeout(CONNECT_TIMEOUT, dc_open_notify.notified()).await;
        tracing::debug!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "dc_open_notify.timeout",
            state = "end",
            result = ?notify_result
        );
        notify_result.map_err(|_| TransportError::Timeout)?;
    } else {
        tracing::debug!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            "data channel already open, skipping notify wait"
        );
    }

    let secure_context = match timeout(Duration::from_secs(10), secure_rx).await {
        Ok(Ok(result)) => result?,
        Ok(Err(_)) => None,
        Err(_) => {
            if secure_transport_active {
                return Err(TransportError::Setup("secure handshake timed out".into()));
            } else {
                None
            }
        }
    };

    if let Some(ref result) = secure_context {
        transport.enable_encryption(result.as_ref())?;
        tracing::debug!(
            target = "webrtc",
            role = "answerer",
            "sending encrypted __ready__ sentinel to offerer"
        );
        if let Err(err) = transport.send_text("__ready__") {
            tracing::warn!(
                target = "webrtc",
                error = %err,
                "answerer encrypted readiness ack failed"
            );
        }
    }
    let transport_dyn: Arc<dyn Transport> = transport.clone();

    Ok(WebRtcConnection::new(
        transport_dyn,
        channels,
        secure_context,
    ))
}

fn endpoint(base: &str, suffix: &str) -> Result<Url, TransportError> {
    let full = format!("{}/{}", base.trim_end_matches('/'), suffix);
    Url::parse(&full).map_err(|err| TransportError::Setup(err.to_string()))
}

fn endpoint_with_params(
    base: &str,
    suffix: &str,
    params: &[(&str, &str)],
) -> Result<Url, TransportError> {
    let mut url = endpoint(base, suffix)?;
    if !params.is_empty() {
        url.query_pairs_mut().extend_pairs(params.iter().cloned());
    }
    Ok(url)
}

async fn post_sdp(
    client: &Client,
    base: &str,
    suffix: &str,
    params: &[(&str, &str)],
    payload: &WebRtcSdpPayload,
) -> Result<(), TransportError> {
    let url = endpoint_with_params(base, suffix, params)?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "start"
    );
    let send_attempt = client.post(url).json(payload).send().await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status)
    );
    let response = send_attempt.map_err(http_error)?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(TransportError::Setup(format!(
            "unexpected signaling status {}",
            response.status()
        )))
    }
}

async fn poll_answer_for_peer(
    client: &Client,
    signaling_base: &str,
    poll_interval: Duration,
    handshake_id: &str,
) -> Result<WebRtcSdpPayload, TransportError> {
    loop {
        let attempt = fetch_sdp(
            client,
            signaling_base,
            "answer",
            &[("handshake_id", handshake_id)],
        )
        .await?;
        if let Some(payload) = attempt {
            return Ok(payload);
        }
        sleep(poll_interval).await;
    }
}

async fn fetch_sdp(
    client: &Client,
    base: &str,
    suffix: &str,
    params: &[(&str, &str)],
) -> Result<Option<WebRtcSdpPayload>, TransportError> {
    let url = endpoint_with_params(base, suffix, params)?;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "start"
    );
    let send_attempt = client.get(url).send().await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status)
    );
    let response = send_attempt.map_err(http_error)?;

    match response.status() {
        StatusCode::OK => {
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "start"
            );
            let payload_attempt = response.json::<WebRtcSdpPayload>().await;
            tracing::debug!(
                target = "beach_human::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "end",
                success = payload_attempt.is_ok()
            );
            let payload = payload_attempt.map_err(http_error)?;
            Ok(Some(payload))
        }
        StatusCode::NOT_FOUND => Ok(None),
        status if status.is_server_error() => Err(TransportError::Setup(format!(
            "signaling server returned {status}"
        ))),
        status => Err(TransportError::Setup(format!(
            "unexpected signaling status {status}"
        ))),
    }
}

fn payload_from_description(
    desc: &RTCSessionDescription,
    handshake_id: &str,
    from_peer: &str,
    to_peer: &str,
    passphrase: Option<&str>,
) -> Result<WebRtcSdpPayload, TransportError> {
    let typ = desc.sdp_type.to_string();
    let mut payload = WebRtcSdpPayload {
        sdp: desc.sdp.clone(),
        typ: typ.clone(),
        handshake_id: handshake_id.to_string(),
        from_peer: from_peer.to_string(),
        to_peer: to_peer.to_string(),
        sealed: None,
    };
    if let Some(passphrase_value) = passphrase {
        if should_encrypt(Some(passphrase_value)) {
            if let Ok(psk) = derive_pre_shared_key(passphrase_value, handshake_id) {
                tracing::debug!(
                    target = "webrtc",
                    handshake_id = %handshake_id,
                    from_peer,
                    to_peer,
                    key = %hex::encode(psk),
                    "offerer derived pre-shared key"
                );
            }
            let label = match desc.sdp_type {
                RTCSdpType::Offer => MessageLabel::Offer,
                RTCSdpType::Answer => MessageLabel::Answer,
                other => {
                    return Err(TransportError::Setup(format!(
                        "unsupported sdp type {other} for secure signaling"
                    )));
                }
            };
            let associated = [from_peer, to_peer, typ.as_str()];
            let sealed = seal_message(
                passphrase_value,
                handshake_id,
                label,
                &associated,
                desc.sdp.as_bytes(),
            )?;
            tracing::debug!(
                target = "webrtc",
                handshake_id = %handshake_id,
                nonce = sealed.nonce,
                ciphertext_len = sealed.ciphertext.len(),
                "offer sealed envelope created"
            );
            payload.sdp.clear();
            payload.sealed = Some(sealed);
        }
    }
    Ok(payload)
}

fn session_description_from_payload(
    payload: &WebRtcSdpPayload,
    passphrase: Option<&str>,
) -> Result<RTCSessionDescription, TransportError> {
    let sdp_type = RTCSdpType::from(payload.typ.as_str());
    let sdp_plain = if let Some(sealed) = &payload.sealed {
        let passphrase_value = passphrase.ok_or_else(|| {
            TransportError::Setup("missing passphrase for sealed signaling payload".into())
        })?;
        let label = match sdp_type {
            RTCSdpType::Offer => MessageLabel::Offer,
            RTCSdpType::Answer => MessageLabel::Answer,
            other => {
                return Err(TransportError::Setup(format!(
                    "unsupported sdp type {other} for sealed payload"
                )));
            }
        };
        let associated = [
            payload.from_peer.as_str(),
            payload.to_peer.as_str(),
            payload.typ.as_str(),
        ];
        let plaintext = open_message(
            passphrase_value,
            &payload.handshake_id,
            label,
            &associated,
            sealed,
        )?;
        String::from_utf8(plaintext)
            .map_err(|err| TransportError::Setup(format!("invalid utf8 in decrypted sdp: {err}")))?
    } else {
        payload.sdp.clone()
    };
    let description = match sdp_type {
        RTCSdpType::Offer => RTCSessionDescription::offer(sdp_plain.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Answer => RTCSessionDescription::answer(sdp_plain.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Pranswer => RTCSessionDescription::pranswer(sdp_plain.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Rollback | RTCSdpType::Unspecified => {
            return Err(TransportError::Setup(format!(
                "unsupported sdp type {}",
                payload.typ
            )));
        }
    };
    Ok(description)
}

fn http_error(err: reqwest::Error) -> TransportError {
    TransportError::Setup(err.to_string())
}

fn resolve_ice_candidate(
    candidate: String,
    sdp_mid: Option<String>,
    sdp_mline_index: Option<u32>,
    sealed: Option<SealedEnvelope>,
    passphrase: Option<&str>,
    handshake_id: &str,
    from_peer: &str,
    to_peer: &str,
) -> Result<IceCandidateBlob, TransportError> {
    if let Some(sealed_env) = sealed {
        let passphrase_value = passphrase.ok_or_else(|| {
            TransportError::Setup("missing passphrase for sealed ice candidate".into())
        })?;
        let associated = [from_peer, to_peer, handshake_id];
        let plaintext = open_message(
            passphrase_value,
            handshake_id,
            MessageLabel::Ice,
            &associated,
            &sealed_env,
        )?;
        let decoded: IceCandidateBlob = serde_json::from_slice(&plaintext).map_err(|err| {
            TransportError::Setup(format!("decode sealed ice candidate failed: {err}"))
        })?;
        Ok(decoded)
    } else {
        Ok(IceCandidateBlob {
            candidate,
            sdp_mid,
            sdp_mline_index,
        })
    }
}

async fn create_webrtc_pair() -> Result<TransportPair, TransportError> {
    // Set up virtual network so tests can run without OS networking access.
    let wan = Arc::new(AsyncMutex::new(
        Router::new(RouterConfig {
            cidr: "10.0.0.0/24".to_owned(),
            ..Default::default()
        })
        .map_err(to_setup_error)?,
    ));

    let offer_vnet = Arc::new(Net::new(Some(NetConfig {
        static_ips: vec!["10.0.0.2".to_owned()],
        ..Default::default()
    })));
    attach_vnet_to_router(&offer_vnet, &wan).await?;

    let answer_vnet = Arc::new(Net::new(Some(NetConfig {
        static_ips: vec!["10.0.0.3".to_owned()],
        ..Default::default()
    })));
    attach_vnet_to_router(&answer_vnet, &wan).await?;

    {
        let mut router = wan.lock().await;
        router.start().await.map_err(to_setup_error)?;
    }

    let mut offer_setting = SettingEngine::default();
    offer_setting.set_vnet(Some(offer_vnet.clone()));
    offer_setting.set_ice_timeouts(
        Some(Duration::from_secs(1)),
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(200)),
    );

    let mut answer_setting = SettingEngine::default();
    answer_setting.set_vnet(Some(answer_vnet.clone()));
    answer_setting.set_ice_timeouts(
        Some(Duration::from_secs(1)),
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(200)),
    );

    let offer_api = build_api(offer_setting)?;
    let answer_api = build_api(answer_setting)?;

    let config = RTCConfiguration::default();

    let offer_pc = Arc::new(
        offer_api
            .new_peer_connection(config.clone())
            .await
            .map_err(to_setup_error)?,
    );
    let answer_pc = Arc::new(
        answer_api
            .new_peer_connection(config)
            .await
            .map_err(to_setup_error)?,
    );

    let dc_init = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };
    let offer_dc = offer_pc
        .create_data_channel("beach-human", Some(dc_init))
        .await
        .map_err(to_setup_error)?;

    let (offer_dc_open_tx, offer_dc_open_rx) = oneshot::channel();
    let offer_dc_open_signal = Arc::new(Mutex::new(Some(offer_dc_open_tx)));
    offer_dc.on_open(Box::new(move || {
        let signal = offer_dc_open_signal.clone();
        Box::pin(async move {
            if let Some(tx) = signal.lock().unwrap().take() {
                let _ = tx.send(());
            }
        })
    }));

    let answer_dc_holder = Arc::new(tokio::sync::Mutex::new(None::<Arc<RTCDataChannel>>));
    let (answer_dc_open_tx, answer_dc_open_rx) = oneshot::channel();
    let answer_dc_signal = Arc::new(Mutex::new(Some(answer_dc_open_tx)));
    let holder_clone = answer_dc_holder.clone();
    let signal_clone = answer_dc_signal.clone();
    answer_pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let holder = holder_clone.clone();
        let signal = signal_clone.clone();
        Box::pin(async move {
            holder.lock().await.replace(dc.clone());
            dc.on_open(Box::new(move || {
                let signal = signal.clone();
                Box::pin(async move {
                    if let Some(tx) = signal.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                })
            }));
        })
    }));

    let (offer_candidate_tx, offer_candidate_rx) =
        tokio_mpsc::unbounded_channel::<Option<RTCIceCandidateInit>>();
    offer_pc.on_ice_candidate(Box::new(move |candidate| {
        let tx = offer_candidate_tx.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = tx.send(Some(json));
                }
            } else {
                let _ = tx.send(None);
            }
        })
    }));

    let (answer_candidate_tx, answer_candidate_rx) =
        tokio_mpsc::unbounded_channel::<Option<RTCIceCandidateInit>>();
    answer_pc.on_ice_candidate(Box::new(move |candidate| {
        let tx = answer_candidate_tx.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = tx.send(Some(json));
                }
            } else {
                let _ = tx.send(None);
            }
        })
    }));

    let offer = offer_pc.create_offer(None).await.map_err(to_setup_error)?;
    offer_pc
        .set_local_description(offer)
        .await
        .map_err(to_setup_error)?;
    wait_for_local_description(&offer_pc).await?;

    let offer_desc = offer_pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing offer description".into()))?;
    answer_pc
        .set_remote_description(offer_desc)
        .await
        .map_err(to_setup_error)?;

    let answer = answer_pc
        .create_answer(None)
        .await
        .map_err(to_setup_error)?;
    answer_pc
        .set_local_description(answer)
        .await
        .map_err(to_setup_error)?;
    wait_for_local_description(&answer_pc).await?;

    let answer_desc = answer_pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing answer description".into()))?;
    offer_pc
        .set_remote_description(answer_desc)
        .await
        .map_err(to_setup_error)?;

    let answer_pc_for_offer = answer_pc.clone();
    spawn_task(async move {
        let mut rx = offer_candidate_rx;
        while let Some(candidate) = rx.recv().await {
            match candidate {
                Some(init) => {
                    let _ = answer_pc_for_offer.add_ice_candidate(init).await;
                }
                None => break,
            }
        }
    });

    let offer_pc_for_answer = offer_pc.clone();
    spawn_task(async move {
        let mut rx = answer_candidate_rx;
        while let Some(candidate) = rx.recv().await {
            match candidate {
                Some(init) => {
                    let _ = offer_pc_for_answer.add_ice_candidate(init).await;
                }
                None => break,
            }
        }
    });

    wait_for_connection(&offer_pc).await?;
    wait_for_connection(&answer_pc).await?;

    timeout(CONNECT_TIMEOUT, offer_dc_open_rx)
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;
    timeout(CONNECT_TIMEOUT, answer_dc_open_rx)
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;

    let answer_dc = answer_dc_holder
        .lock()
        .await
        .clone()
        .ok_or_else(|| TransportError::Setup("answer data channel missing".into()))?;

    let client_id = next_transport_id();
    let server_id = next_transport_id();

    let router_keepalive = Some(wan.clone());

    let client_transport = WebRtcTransport::new(
        TransportKind::WebRtc,
        client_id,
        server_id,
        offer_pc.clone(),
        offer_dc.clone(),
        router_keepalive.clone(),
        None,
        None,
        None,
    );

    let server_transport = WebRtcTransport::new(
        TransportKind::WebRtc,
        server_id,
        client_id,
        answer_pc.clone(),
        answer_dc.clone(),
        router_keepalive,
        None,
        None,
        None,
    );

    Ok(TransportPair {
        client: Box::new(client_transport),
        server: Box::new(server_transport),
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub async fn create_test_pair() -> Result<TransportPair, TransportError> {
    create_webrtc_pair().await
}

async fn wait_for_local_description(pc: &Arc<RTCPeerConnection>) -> Result<(), TransportError> {
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "start"
    );
    let already_present = pc.local_description().await.is_some();
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "end",
        has_description = already_present
    );
    if already_present {
        return Ok(());
    }
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "start"
    );
    let mut gather = pc.gathering_complete_promise().await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "end"
    );
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "start"
    );
    let _ = gather.recv().await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "end"
    );
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.final",
        state = "start"
    );
    let final_present = pc.local_description().await.is_some();
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.final",
        state = "end",
        has_description = final_present
    );
    if final_present {
        Ok(())
    } else {
        Err(TransportError::Setup(
            "failed to obtain local description".into(),
        ))
    }
}

async fn wait_for_connection(pc: &Arc<RTCPeerConnection>) -> Result<(), TransportError> {
    if pc.connection_state() == RTCPeerConnectionState::Connected {
        return Ok(());
    }
    let (tx, rx) = oneshot::channel();
    let signal = Arc::new(Mutex::new(Some(tx)));
    let signal_clone = signal.clone();
    pc.on_peer_connection_state_change(Box::new(move |state| {
        let signal = signal_clone.clone();
        Box::pin(async move {
            tracing::debug!(target = "webrtc", ?state, "peer connection state changed");
            if state == RTCPeerConnectionState::Connected {
                if let Some(tx) = signal.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        })
    }));

    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_connection",
        await = "timeout(rx)",
        state = "start"
    );
    let wait_result = timeout(CONNECT_TIMEOUT, rx).await;
    tracing::debug!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_connection",
        await = "timeout(rx)",
        state = "end",
        result = ?wait_result
    );
    wait_result
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;
    Ok(())
}

fn to_setup_error<E: std::fmt::Display>(err: E) -> TransportError {
    TransportError::Setup(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn webrtc_pair_round_trip() {
        let pair = match build_pair() {
            Ok(pair) => pair,
            Err(err) => {
                tracing::debug!(target = "webrtc", error = %err, "skipping webrtc_pair_round_trip");
                return;
            }
        };
        let timeout = Duration::from_secs(5);

        let client = &pair.client;
        let server = &pair.server;

        assert_eq!(client.kind(), TransportKind::WebRtc);
        assert_eq!(server.kind(), TransportKind::WebRtc);

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

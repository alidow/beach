use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use crossbeam_channel::{
    Receiver as CrossbeamReceiver, RecvTimeoutError as CrossbeamRecvTimeoutError,
    Sender as CrossbeamSender, TryRecvError as CrossbeamTryRecvError,
    unbounded as crossbeam_unbounded,
};
use if_addrs::get_if_addrs;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use once_cell::sync::{Lazy, OnceCell as SyncOnceCell};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Mutex as AsyncMutex, Notify, OnceCell, SetError, mpsc as tokio_mpsc, oneshot};
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
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::util::vnet::net::{Net, NetConfig};
use webrtc::util::vnet::router::{Router, RouterConfig};
use webrtc_ice::{
    network_type::NetworkType,
    udp_network::{EphemeralUDP, UDPNetwork},
};

use crate::auth;
use crate::auth::error::AuthError;
use crate::auth::gate::TurnIceServer;
use crate::metrics;
use crate::server::terminal::host::{
    CONTROLLER_ACK_CHANNEL_LABEL, CONTROLLER_CHANNEL_LABEL, CONTROLLER_STATE_CHANNEL_LABEL,
    LEGACY_CONTROLLER_CHANNEL_LABEL,
};
use crate::transport::framed;
use crate::transport::webrtc::signaling::PeerInfo;
use crate::transport::{
    Transport, TransportError, TransportId, TransportKind, TransportMessage, TransportPair,
    decode_message, encode_message, next_transport_id,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READY_ACK_POLL_ATTEMPTS: usize = 200;
const READY_ACK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MCP_CHANNEL_LABEL: &str = "mcp-jsonrpc";
const CONTROL_PRIORITY_NAMESPACES: &[&str] = &["controller"];
mod secure_handshake;
mod secure_signaling;
mod signaling;
pub use signaling::SignalingClient;

static OFFER_ENCRYPTION_DELAY_MS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct IceCandidateInfo {
    ip: IpAddr,
    port: u16,
    scope: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutboundPriority {
    High,
    Low,
}

impl OutboundPriority {
    fn as_label(&self) -> &'static str {
        match self {
            OutboundPriority::High => "high",
            OutboundPriority::Low => "low",
        }
    }
}

#[derive(Debug)]
struct OutboundFrame {
    bytes: Vec<u8>,
    namespace: String,
    priority: OutboundPriority,
    enqueued_at: Instant,
}

fn classify_priority(namespace: &str, payload_len: usize) -> OutboundPriority {
    if CONTROL_PRIORITY_NAMESPACES.contains(&namespace) || payload_len <= 512 {
        OutboundPriority::High
    } else {
        OutboundPriority::Low
    }
}

#[derive(Default)]
struct OutboundQueueDepth {
    per_namespace: HashMap<String, (usize, usize)>,
}

impl OutboundQueueDepth {
    fn increment(&mut self, namespace: &str, priority: OutboundPriority) {
        let snapshot = {
            let entry = self
                .per_namespace
                .entry(namespace.to_string())
                .or_insert((0, 0));
            match priority {
                OutboundPriority::High => entry.0 = entry.0.saturating_add(1),
                OutboundPriority::Low => entry.1 = entry.1.saturating_add(1),
            }
            *entry
        };
        self.update_gauges(namespace, snapshot);
    }

    fn decrement(&mut self, namespace: &str, priority: OutboundPriority) {
        if let Some(snapshot) = self.per_namespace.get_mut(namespace).map(|entry| {
            match priority {
                OutboundPriority::High => entry.0 = entry.0.saturating_sub(1),
                OutboundPriority::Low => entry.1 = entry.1.saturating_sub(1),
            }
            *entry
        }) {
            self.update_gauges(namespace, snapshot);
        }
    }

    fn update_gauges(&self, namespace: &str, entry: (usize, usize)) {
        metrics::FRAMED_OUTBOUND_QUEUE_DEPTH
            .with_label_values(&[namespace, "high"])
            .set(entry.0 as i64);
        metrics::FRAMED_OUTBOUND_QUEUE_DEPTH
            .with_label_values(&[namespace, "low"])
            .set(entry.1 as i64);
    }
}

fn classify_candidate_scope(ip: &IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                "loopback"
            } else if v4.is_private() {
                "private"
            } else if v4.is_link_local() {
                "link_local"
            } else if v4.is_multicast() {
                "multicast"
            } else {
                "public"
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                "loopback"
            } else if v6.is_unique_local() {
                "private"
            } else if v6.is_unicast_link_local() {
                "link_local"
            } else if v6.is_multicast() {
                "multicast"
            } else {
                "public"
            }
        }
    }
}

fn parse_candidate_info(candidate: &str) -> Option<IceCandidateInfo> {
    let parts: Vec<_> = candidate.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    let ip = parts.get(4)?.parse::<IpAddr>().ok()?;
    let port = parts.get(5)?.parse::<u16>().ok()?;
    Some(IceCandidateInfo {
        scope: classify_candidate_scope(&ip),
        ip,
        port,
    })
}

#[derive(Clone, Copy, Debug)]
enum NatHintSource {
    EnvIp,
    EnvHost,
    AutoDetect,
}

static NAT_HINT: SyncOnceCell<Option<(String, NatHintSource)>> = SyncOnceCell::new();

fn nat_ip_hint() -> Option<&'static (String, NatHintSource)> {
    NAT_HINT
        .get_or_init(|| {
            let hint = detect_nat_ip_hint();
            match &hint {
                Some((ip, source)) => {
                    tracing::info!(
                        target = "beach::transport::webrtc",
                        nat_ip = %ip,
                        nat_hint_source = ?source,
                        "using NAT 1:1 hint for WebRTC"
                    );
                }
                None => {
                    tracing::debug!(
                        target = "beach::transport::webrtc",
                        "no NAT hint detected for WebRTC"
                    );
                }
            }
            hint
        })
        .as_ref()
}

fn detect_nat_ip_hint() -> Option<(String, NatHintSource)> {
    if let Some(ip) = env_nat_ip() {
        return Some((ip, NatHintSource::EnvIp));
    }
    if let Some(ip) = env_nat_host() {
        return Some((ip, NatHintSource::EnvHost));
    }
    if !auth::is_public_mode() {
        return None;
    }
    detect_lan_ipv4().map(|ip| (ip.to_string(), NatHintSource::AutoDetect))
}

fn env_nat_ip() -> Option<String> {
    std::env::var("BEACH_ICE_PUBLIC_IP")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_nat_host() -> Option<String> {
    let host = std::env::var("BEACH_ICE_PUBLIC_HOST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    (host.as_str(), 0)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.find(|addr| matches!(addr, std::net::SocketAddr::V4(_))))
        .map(|addr| addr.ip().to_string())
}

fn detect_lan_ipv4() -> Option<Ipv4Addr> {
    let addrs = get_if_addrs().ok()?;
    let mut fallback = None;
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        if let IpAddr::V4(addr) = iface.ip() {
            if should_skip_addr(&addr) {
                continue;
            }
            if addr.is_private() {
                return Some(addr);
            }
            fallback.get_or_insert(addr);
        }
    }
    fallback
}

fn should_skip_addr(addr: &Ipv4Addr) -> bool {
    let octets = addr.octets();
    if octets[0] == 0 || octets[0] == 127 {
        return true;
    }
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    if octets[0] == 192 && octets[1] == 168 && octets[2] == 65 {
        return true;
    }
    if octets[0] == 172 && (10..20).contains(&octets[1]) {
        return true;
    }
    false
}

fn apply_nat_hint(setting: &mut SettingEngine) {
    if let Some((ip, _source)) = nat_ip_hint() {
        setting.set_nat_1to1_ips(vec![ip.clone()], RTCIceCandidateType::Host);
    }

    if let Some((start, end)) = ice_port_range_hint() {
        match EphemeralUDP::new(start, end) {
            Ok(range) => {
                tracing::info!(
                    target = "beach::transport::webrtc",
                    port_start = start,
                    port_end = end,
                    "using ICE UDP port range from env"
                );
                setting.set_udp_network(UDPNetwork::Ephemeral(range));
            }
            Err(err) => {
                tracing::warn!(
                    target = "beach::transport::webrtc",
                    port_start = start,
                    port_end = end,
                    error = %err,
                    "invalid ICE UDP port range; falling back to defaults"
                );
            }
        }
    }
}

fn ice_port_range_hint() -> Option<(u16, u16)> {
    let start = std::env::var("BEACH_ICE_PORT_START").ok()?;
    let end = std::env::var("BEACH_ICE_PORT_END").ok()?;
    let start = match start.trim().parse::<u16>() {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                target = "beach::transport::webrtc",
                error = %err,
                value = %start,
                "failed to parse BEACH_ICE_PORT_START"
            );
            return None;
        }
    };
    let end = match end.trim().parse::<u16>() {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                target = "beach::transport::webrtc",
                error = %err,
                value = %end,
                "failed to parse BEACH_ICE_PORT_END"
            );
            return None;
        }
    };
    if start > end {
        tracing::warn!(
            target = "beach::transport::webrtc",
            port_start = start,
            port_end = end,
            "ignoring ICE port range because start > end"
        );
        return None;
    }
    Some((start, end))
}

pub struct OfferEncryptionDelayGuard {
    previous: u64,
}

pub fn install_offer_encryption_delay(delay: Option<Duration>) -> OfferEncryptionDelayGuard {
    let millis = delay
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let previous = OFFER_ENCRYPTION_DELAY_MS.swap(millis, Ordering::SeqCst);
    OfferEncryptionDelayGuard { previous }
}

impl Drop for OfferEncryptionDelayGuard {
    fn drop(&mut self) {
        OFFER_ENCRYPTION_DELAY_MS.store(self.previous, Ordering::SeqCst);
    }
}

fn offer_encryption_delay() -> Option<Duration> {
    let millis = OFFER_ENCRYPTION_DELAY_MS.load(Ordering::SeqCst);
    if millis == 0 {
        None
    } else {
        Some(Duration::from_millis(millis))
    }
}

use secure_handshake::{
    HANDSHAKE_CHANNEL_LABEL, HandshakeInbox, HandshakeParams, HandshakeResult, HandshakeRole,
    build_prologue_context, handshake_channel_init, hex_preview, run_handshake,
    secure_transport_enabled,
};
use secure_signaling::{
    MessageLabel, SealedEnvelope, derive_handshake_key_from_session, derive_pre_shared_key,
    open_message, open_message_with_psk, seal_message, seal_message_with_psk, should_encrypt,
};
use signaling::{PeerRole, RemotePeerEvent, RemotePeerJoined, WebRTCSignal};

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

#[derive(Debug, Clone, Copy)]
enum IceServerSource {
    LocalOverride,
    BeachGateTurn,
    PublicStun,
    HostOnly,
}

impl IceServerSource {
    fn as_str(&self) -> &'static str {
        match self {
            IceServerSource::LocalOverride => "env_override",
            IceServerSource::BeachGateTurn => "beach_gate_turn",
            IceServerSource::PublicStun => "public_stun",
            IceServerSource::HostOnly => "host_only",
        }
    }
}

#[derive(Debug, Clone)]
struct IceServerSelection {
    source: IceServerSource,
    servers: Option<Vec<webrtc::ice_transport::ice_server::RTCIceServer>>,
}

impl IceServerSelection {
    fn with_source(
        source: IceServerSource,
        servers: Option<Vec<webrtc::ice_transport::ice_server::RTCIceServer>>,
    ) -> Self {
        Self { source, servers }
    }

    fn server_count(&self) -> usize {
        self.servers
            .as_ref()
            .map(|servers| servers.len())
            .unwrap_or(0)
    }
}

const TRANSPORT_ENCRYPTION_VERSION: u8 = 1;
const TRANSPORT_ENCRYPTION_AAD: &[u8] = b"beach:secure-transport:v1";

fn default_stun_server() -> webrtc::ice_transport::ice_server::RTCIceServer {
    webrtc::ice_transport::ice_server::RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    }
}

fn log_ice_configuration(role: &'static str, selection: &IceServerSelection, stun_added: bool) {
    let mode = if auth::is_public_mode() {
        "public"
    } else {
        "managed"
    };
    tracing::info!(
        target = "beach::transport::webrtc",
        peer_role = role,
        cli_mode = mode,
        ice_source = selection.source.as_str(),
        configured_servers = selection.server_count(),
        default_stun_appended = stun_added,
        "configuring WebRTC peer connection"
    );
}

fn local_ice_servers_from_env()
-> Result<Option<Vec<webrtc::ice_transport::ice_server::RTCIceServer>>, AuthError> {
    let raw = match std::env::var("BEACH_ICE_SERVERS") {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let overrides: Vec<TurnIceServer> = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(err) => {
            tracing::error!(
                target: "beach::transport::webrtc",
                error = %err,
                "failed to parse BEACH_ICE_SERVERS"
            );
            return Err(AuthError::Other(format!(
                "invalid BEACH_ICE_SERVERS value: {err}"
            )));
        }
    };

    let mut servers = Vec::new();
    for server in overrides.into_iter() {
        if server.urls.is_empty() {
            continue;
        }
        servers.push(webrtc::ice_transport::ice_server::RTCIceServer {
            urls: server.urls,
            username: server.username.unwrap_or_default(),
            credential: server.credential.unwrap_or_default(),
            ..Default::default()
        });
    }

    if servers.is_empty() {
        tracing::error!(
            target: "beach::transport::webrtc",
            "BEACH_ICE_SERVERS was set but did not include any urls"
        );
        return Err(AuthError::Other(
            "BEACH_ICE_SERVERS did not include any urls".into(),
        ));
    }

    tracing::info!(
        target: "beach::transport::webrtc",
        server_count = servers.len(),
        "using ICE servers from BEACH_ICE_SERVERS"
    );
    Ok(Some(servers))
}

async fn load_turn_ice_servers() -> Result<IceServerSelection, AuthError> {
    if let Some(servers) = local_ice_servers_from_env()? {
        return Ok(IceServerSelection::with_source(
            IceServerSource::LocalOverride,
            Some(servers),
        ));
    }

    if auth::is_public_mode() {
        tracing::info!(
            target: "beach::transport::webrtc",
            "public mode: using default public STUN server"
        );
        return Ok(IceServerSelection::with_source(
            IceServerSource::PublicStun,
            Some(vec![default_stun_server()]),
        ));
    }

    match auth::resolve_turn_credentials(None).await {
        Ok(credentials) => {
            let realm = credentials.realm.clone();
            let ttl = credentials.ttl_seconds;
            let servers: Vec<_> = credentials
                .ice_servers
                .into_iter()
                .filter_map(|server| {
                    let urls = server.urls;
                    if urls.is_empty() {
                        return None;
                    }
                    let username = server.username.unwrap_or_default();
                    let credential = server.credential.unwrap_or_default();
                    Some(webrtc::ice_transport::ice_server::RTCIceServer {
                        urls,
                        username,
                        credential,
                    })
                })
                .collect();
            if servers.is_empty() {
                tracing::warn!(
                    target: "beach::transport::webrtc",
                    realm = %realm,
                    "TURN credentials returned no ICE servers"
                );
                Ok(IceServerSelection::with_source(
                    IceServerSource::HostOnly,
                    None,
                ))
            } else {
                tracing::debug!(
                    target: "beach::transport::webrtc",
                    realm = %realm,
                    ttl_seconds = ttl,
                    server_count = servers.len(),
                    "using TURN credentials from Beach Gate"
                );
                Ok(IceServerSelection::with_source(
                    IceServerSource::BeachGateTurn,
                    Some(servers),
                ))
            }
        }
        Err(err @ AuthError::TurnNotEntitled) => Err(err),
        Err(err) => {
            match err {
                AuthError::NotLoggedIn | AuthError::ProfileNotFound(_) => {
                    tracing::debug!(
                        target: "beach::transport::webrtc",
                        error = %err,
                        "TURN credentials unavailable"
                    );
                }
                _ => {
                    tracing::warn!(
                        target: "beach::transport::webrtc",
                        error = %err,
                        "failed to fetch TURN credentials"
                    );
                }
            }
            Ok(IceServerSelection::with_source(
                IceServerSource::HostOnly,
                None,
            ))
        }
    }
}

struct EncryptionState {
    send_cipher: ChaCha20Poly1305,
    recv_cipher: ChaCha20Poly1305,
    send_counter: AtomicU64,
    recv_counter: AtomicU64,
    send_lock: Mutex<()>,
    recv_lock: Mutex<()>,
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
        if self.enabled.load(Ordering::SeqCst) {
            true
        } else {
            let guard = self.state.lock().unwrap();
            guard.is_some()
        }
    }

    fn counters(&self) -> Option<(u64, u64)> {
        let guard = self.state.lock().unwrap();
        guard.as_ref().map(|state| {
            (
                state.send_counter.load(Ordering::SeqCst),
                state.recv_counter.load(Ordering::SeqCst),
            )
        })
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
            send_lock: Mutex::new(()),
            recv_lock: Mutex::new(()),
        });
        self.enabled.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, TransportError> {
        let guard = self.state.lock().unwrap();
        let state = guard
            .as_ref()
            .ok_or_else(|| TransportError::Setup("secure transport not negotiated".into()))?;
        let _lock = state
            .send_lock
            .lock()
            .map_err(|_| TransportError::Setup("secure transport send lock poisoned".into()))?;
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
        tracing::trace!(
            target = "beach::transport::webrtc::crypto",
            direction = "send",
            counter,
            plaintext_len = plaintext.len(),
            frame_len = frame.len(),
            "encrypted payload"
        );
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
        let _lock = state
            .recv_lock
            .lock()
            .map_err(|_| TransportError::Setup("secure transport recv lock poisoned".into()))?;
        let mut counter_bytes = [0u8; 8];
        counter_bytes.copy_from_slice(&frame[1..9]);
        let counter = u64::from_be_bytes(counter_bytes);
        let previous = state.recv_counter.swap(counter, Ordering::SeqCst);
        if counter != previous {
            tracing::debug!(
                target = "beach::transport::webrtc::crypto",
                direction = "recv",
                expected_counter = previous,
                received_counter = counter,
                frame_len = frame.len(),
                "secure transport counter resynchronised"
            );
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
        state
            .recv_counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| counter.checked_add(1))
            .ok();
        tracing::trace!(
            target = "beach::transport::webrtc::crypto",
            direction = "recv",
            counter,
            ciphertext_len = frame.len().saturating_sub(9),
            plaintext_len = plaintext.len(),
            "decrypted payload"
        );
        Ok(plaintext)
    }
}

fn nonce_from_counter(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

fn looks_like_encrypted_frame(bytes: &[u8]) -> bool {
    bytes.len() >= 9 && bytes[0] == TRANSPORT_ENCRYPTION_VERSION
}

#[derive(Clone)]
pub struct WebRtcConnection {
    transport: Arc<dyn Transport>,
    channels: WebRtcChannels,
    secure: Option<Arc<HandshakeResult>>,
    signaling_client: Option<Arc<SignalingClient>>,
    metadata: Option<HashMap<String, String>>,
}

impl WebRtcConnection {
    pub fn new(
        transport: Arc<dyn Transport>,
        channels: WebRtcChannels,
        secure: Option<Arc<HandshakeResult>>,
        signaling_client: Option<Arc<SignalingClient>>,
        metadata: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            transport,
            channels,
            secure,
            signaling_client,
            metadata,
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

    pub fn signaling_client(&self) -> Option<Arc<SignalingClient>> {
        self.signaling_client.clone()
    }

    pub fn metadata(&self) -> Option<HashMap<String, String>> {
        self.metadata.clone()
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
    outbound_seq: Mutex<HashMap<String, u64>>,
    outbound_high_tx: tokio_mpsc::UnboundedSender<OutboundFrame>,
    outbound_low_tx: tokio_mpsc::UnboundedSender<OutboundFrame>,
    inbound_tx: CrossbeamSender<TransportMessage>,
    inbound_rx: Mutex<CrossbeamReceiver<TransportMessage>>,
    _pc: Arc<RTCPeerConnection>,
    _dc: Arc<RTCDataChannel>,
    _router: Option<Arc<AsyncMutex<Router>>>,
    _signaling: Option<Arc<SignalingClient>>,
    encryption: Arc<EncryptionManager>,
    pending_encrypted: Arc<Mutex<VecDeque<Vec<u8>>>>,
    frame_config: framed::FramingConfig,
    frame_reassembler: Arc<Mutex<framed::FramedDecoder>>,
    chunk_log_once: AtomicBool,
    raw_mode: bool,
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
        raw_mode: bool,
    ) -> Self {
        let (inbound_tx_raw, inbound_rx) = crossbeam_unbounded();
        let handler_id = id;
        let inbound_tx_for_handler = inbound_tx_raw.clone();
        tracing::debug!(target = "webrtc", transport_id = ?handler_id, "registering data channel handler");
        let encryption = Arc::new(EncryptionManager::new());
        let encryption_clone_for_handler = Arc::clone(&encryption);
        let pending_encrypted = Arc::new(Mutex::new(VecDeque::new()));
        let pending_for_handler = Arc::clone(&pending_encrypted);
        let frame_config = framed::runtime_config().clone();
        let frame_reassembler =
            Arc::new(Mutex::new(framed::FramedDecoder::new(frame_config.clone())));
        let frame_reassembler_for_handler = Arc::clone(&frame_reassembler);
        dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let sender = inbound_tx_for_handler.clone();
            let log_id = handler_id;
            let encryption = Arc::clone(&encryption_clone_for_handler);
            let pending = Arc::clone(&pending_for_handler);
            let frame_reassembler = Arc::clone(&frame_reassembler_for_handler);
            Box::pin(async move {
                let bytes = msg.data.to_vec();
                Self::handle_incoming_bytes(
                    &encryption,
                    &pending,
                    &frame_reassembler,
                    bytes,
                    &sender,
                    log_id,
                    raw_mode,
                );
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

                            transport_id = ?log_id,
                            pc_state = ?pc_state,
                            ice_state = ?ice_state,
                            "data channel closed before readiness handshake completed"
                        );
                        return;
                    }
                }
                tracing::debug!(

                    transport_id = ?log_id,
                    pc_state = ?pc_state,
                    ice_state = ?ice_state,
                    "data channel closed"
                );
            })
        }));

        let (outbound_high_tx, mut outbound_high_rx) =
            tokio_mpsc::unbounded_channel::<OutboundFrame>();
        let (outbound_low_tx, mut outbound_low_rx) =
            tokio_mpsc::unbounded_channel::<OutboundFrame>();
        let dc_clone = dc.clone();
        let transport_id = id;
        let dc_ready_signal = dc_ready.clone();
        let frame_config_for_sender = frame_config.clone();
        spawn_on_global(async move {
            if let Some(notify) = dc_ready_signal {
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "start"
                );
                notify.notified().await;
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "end"
                );
                tracing::debug!(target = "webrtc", transport_id = ?transport_id, "dc ready triggered");
            } else {
                tracing::debug!(target = "webrtc", transport_id = ?transport_id, "dc ready immediate");
            }
            tracing::info!(
                target = "beach::transport::webrtc",
                transport_id = ?transport_id,
                label = %dc_clone.label(),
                peer = ?peer,
                "data channel created"
            );
            tracing::debug!(target = "webrtc", transport_id = ?transport_id, "sender loop start");
            let mut depth = OutboundQueueDepth::default();
            let mut pending_high = VecDeque::new();
            let mut pending_low = VecDeque::new();
            let mut last_priority_log = Instant::now();
            loop {
                if pending_high.is_empty() && pending_low.is_empty() {
                    tracing::debug!(
                        target = "beach::transport::webrtc",
                        transport_id = ?transport_id,
                        await = "outbound_rx.recv",
                        state = "start"
                    );
                    tokio::select! {
                        maybe_frame = outbound_high_rx.recv() => {
                            tracing::debug!(
                                target = "beach::transport::webrtc",
                                transport_id = ?transport_id,
                                await = "outbound_high_rx.recv",
                                state = "end",
                                has_bytes = maybe_frame.is_some()
                            );
                            if let Some(frame) = maybe_frame {
                                depth.increment(&frame.namespace, frame.priority);
                                pending_high.push_back(frame);
                            }
                        }
                        maybe_frame = outbound_low_rx.recv() => {
                            tracing::debug!(
                                target = "beach::transport::webrtc",
                                transport_id = ?transport_id,
                                await = "outbound_low_rx.recv",
                                state = "end",
                                has_bytes = maybe_frame.is_some()
                            );
                            if let Some(frame) = maybe_frame {
                                depth.increment(&frame.namespace, frame.priority);
                                pending_low.push_back(frame);
                            }
                        }
                    }
                } else {
                    while let Ok(frame) = outbound_high_rx.try_recv() {
                        depth.increment(&frame.namespace, frame.priority);
                        pending_high.push_back(frame);
                    }
                    while let Ok(frame) = outbound_low_rx.try_recv() {
                        depth.increment(&frame.namespace, frame.priority);
                        pending_low.push_back(frame);
                    }
                }

                let next = if let Some(frame) = pending_high.pop_front() {
                    if !pending_low.is_empty()
                        && last_priority_log.elapsed() > Duration::from_secs(1)
                    {
                        tracing::info!(
                            target = "beach::transport::webrtc",
                            transport_id = ?transport_id,
                            low_queue_depth = pending_low.len(),
                            "prioritizing control frame over queued payloads"
                        );
                        last_priority_log = Instant::now();
                    }
                    frame
                } else if let Some(frame) = pending_low.pop_front() {
                    frame
                } else {
                    if outbound_high_rx.is_closed() && outbound_low_rx.is_closed() {
                        break;
                    }
                    continue;
                };

                depth.decrement(&next.namespace, next.priority);
                metrics::FRAMED_OUTBOUND_QUEUE_LATENCY
                    .with_label_values(&[&next.namespace, next.priority.as_label()])
                    .observe(next.enqueued_at.elapsed().as_secs_f64() * 1000.0);
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    queued_len = next.bytes.len(),
                    namespace = %next.namespace,
                    priority = %next.priority.as_label(),
                    "dequeued outbound"
                );
                let data = Bytes::from(next.bytes);
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "start"
                );
                let before = dc_clone.buffered_amount().await;
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "end",
                    buffered_before = before
                );
                let budget = frame_config_for_sender.backpressure_budget();
                let mut buffered = before as u64;
                while buffered > budget {
                    tracing::debug!(
                        target = "beach::transport::webrtc",
                        transport_id = ?transport_id,
                        buffered_amount = buffered,
                        budget,
                        "delaying send due to buffered amount"
                    );
                    sleep(Duration::from_millis(5)).await;
                    buffered = dc_clone.buffered_amount().await as u64;
                }
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "start",
                    payload_len = data.len()
                );
                let send_result = timeout(CONNECT_TIMEOUT, dc_clone.send(&data)).await;
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "end",
                    result = ?send_result
                );
                match send_result {
                    Ok(Ok(bytes_written)) => {
                        tracing::debug!(
                            target = "beach::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "start"
                        );
                        let after = dc_clone.buffered_amount().await;
                        tracing::debug!(
                            target = "beach::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "end",
                            buffered_after = after
                        );
                        tracing::debug!(

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

                                transport_id = ?transport_id,
                                error = %err_display,
                                "dropping outbound frame: data channel not open"
                            );
                        } else {
                            tracing::warn!(

                                transport_id = ?transport_id,
                                error = %err_display,
                                "webrtc send error"
                            );
                        }
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(

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
            outbound_seq: Mutex::new(HashMap::new()),
            outbound_high_tx,
            outbound_low_tx,
            inbound_tx: inbound_tx_raw,
            inbound_rx: Mutex::new(inbound_rx),
            _pc: pc,
            _dc: dc,
            _router: router,
            _signaling: signaling,
            encryption,
            pending_encrypted,
            frame_config,
            frame_reassembler,
            chunk_log_once: AtomicBool::new(false),
            raw_mode,
        }
    }

    fn next_seq(&self, namespace: &str) -> u64 {
        let mut guard = self.outbound_seq.lock().unwrap();
        let entry = guard.entry(namespace.to_string()).or_insert(0);
        let seq = *entry;
        *entry = entry.saturating_add(1);
        seq
    }

    fn enable_encryption(&self, result: &HandshakeResult) -> Result<(), TransportError> {
        self.encryption.enable(result)?;
        self.flush_pending_encrypted();
        Ok(())
    }

    fn flush_pending_encrypted(&self) {
        Self::flush_pending_encrypted_internal(
            &self.encryption,
            &self.frame_reassembler,
            &self.pending_encrypted,
            &self.inbound_tx,
            self.id,
            self.raw_mode,
        );
    }

    fn flush_pending_encrypted_internal(
        encryption: &Arc<EncryptionManager>,
        frame_reassembler: &Arc<Mutex<framed::FramedDecoder>>,
        pending: &Arc<Mutex<VecDeque<Vec<u8>>>>,
        sender: &CrossbeamSender<TransportMessage>,
        log_id: TransportId,
        raw_mode: bool,
    ) {
        let mut pending = pending.lock().unwrap();
        if pending.is_empty() {
            return;
        }
        tracing::debug!(
            target = "beach::transport::webrtc::crypto",
            transport_id = ?log_id,
            frames = pending.len(),
            "flushing encrypted frames queued before enable"
        );
        while let Some(bytes) = pending.pop_front() {
            Self::process_incoming_frame(
                encryption,
                frame_reassembler,
                bytes,
                sender,
                log_id,
                raw_mode,
            );
        }
    }

    fn handle_incoming_bytes(
        encryption: &Arc<EncryptionManager>,
        pending: &Arc<Mutex<VecDeque<Vec<u8>>>>,
        frame_reassembler: &Arc<Mutex<framed::FramedDecoder>>,
        bytes: Vec<u8>,
        sender: &CrossbeamSender<TransportMessage>,
        log_id: TransportId,
        raw_mode: bool,
    ) {
        if !encryption.is_enabled() && looks_like_encrypted_frame(&bytes) {
            let mut queue = pending.lock().unwrap();
            queue.push_back(bytes);
            tracing::debug!(
                target = "beach::transport::webrtc::crypto",
                transport_id = ?log_id,
                queued_frames = queue.len(),
                "queued encrypted frame until transport keys are installed"
            );
            return;
        }
        Self::process_incoming_frame(
            encryption,
            frame_reassembler,
            bytes,
            sender,
            log_id,
            raw_mode,
        );
    }

    fn process_incoming_frame(
        encryption: &Arc<EncryptionManager>,
        frame_reassembler: &Arc<Mutex<framed::FramedDecoder>>,
        bytes: Vec<u8>,
        sender: &CrossbeamSender<TransportMessage>,
        log_id: TransportId,
        raw_mode: bool,
    ) {
        let payload = if encryption.is_enabled() {
            match encryption.decrypt(&bytes) {
                Ok(plaintext) => plaintext,
                Err(err) => {
                    tracing::warn!(
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
        let now = Instant::now();
        let mut reassembler = frame_reassembler.lock().unwrap();
        match reassembler.ingest(&payload, now) {
            Ok(Some(frame)) => {
                drop(reassembler);
                Self::forward_framed_payload(
                    frame,
                    sender,
                    log_id,
                    encryption.is_enabled(),
                    encryption.counters(),
                    raw_mode,
                );
            }
            Ok(None) => {
                drop(reassembler);
            }
            Err(err) => {
                drop(reassembler);
                Self::log_framed_error(err, "recv", log_id);
            }
        }
    }

    fn forward_framed_payload(
        frame: framed::FramedMessage,
        sender: &CrossbeamSender<TransportMessage>,
        log_id: TransportId,
        encryption_enabled: bool,
        counters: Option<(u64, u64)>,
        raw_mode: bool,
    ) {
        framed::publish(log_id, frame.clone());
        if frame.namespace != "sync" {
            tracing::debug!(
                transport_id = ?log_id,
                namespace = %frame.namespace,
                kind = %frame.kind,
                len = frame.payload.len(),
                "delivered framed namespace message to subscribers"
            );
            return;
        }
        let payload = frame.payload.to_vec();
        if raw_mode {
            // In raw mode, we treat the payload as either text or binary without expecting
            // the standard framing header.
            let message = match String::from_utf8(payload.clone()) {
                Ok(text) => TransportMessage::text(0, text),
                Err(_) => TransportMessage::binary(0, payload.clone()),
            };
            tracing::debug!(
                transport_id = ?log_id,
                frame_len = payload.len(),
                is_text = message.payload.as_text().is_some(),
                "received raw frame"
            );
            if let Err(err) = sender.send(message) {
                tracing::warn!(
                    transport_id = ?log_id,
                    error = %err,
                    "failed to forward raw inbound frame"
                );
            }
            return;
        }

        if let Some(message) = decode_message(&payload) {
            tracing::debug!(
                transport_id = ?log_id,
                frame_len = payload.len(),
                sequence = message.sequence,
                namespace = %frame.namespace,
                kind = %frame.kind,
                "received frame"
            );
            if let Err(err) = sender.send(message) {
                tracing::warn!(
                    transport_id = ?log_id,
                    error = %err,
                    "failed to enqueue inbound message"
                );
            }
        } else {
            if tracing::enabled!(tracing::Level::TRACE) {
                let preview_len = payload.len().min(32);
                let preview = hex::encode(&payload[..preview_len]);
                let (send_counter, recv_counter) = counters.unwrap_or((0, 0));
                tracing::trace!(
                    target = "beach::transport::webrtc::crypto",
                    transport_id = ?log_id,
                    encryption_enabled,
                    send_counter,
                    recv_counter,
                    frame_len = payload.len(),
                    payload_preview = %preview,
                    payload_preview_len = preview_len,
                    "inbound frame failed decode"
                );
            }
            tracing::warn!(
                transport_id = ?log_id,
                frame_len = payload.len(),
                encryption_enabled,
                "failed to decode message"
            );
        }
    }

    fn log_framed_error(err: framed::FramingError, direction: &str, log_id: TransportId) {
        let reason = match err {
            framed::FramingError::NamespaceTooLong
            | framed::FramingError::KindTooLong
            | framed::FramingError::PayloadTooLarge(_) => "oversized",
            framed::FramingError::UnsupportedVersion(_) | framed::FramingError::Malformed(_) => {
                "malformed"
            }
            framed::FramingError::CrcMismatch => "crc_failures",
            framed::FramingError::MacMissing
            | framed::FramingError::UnknownMacKey(_)
            | framed::FramingError::MacMismatch => "mac_failures",
        };
        metrics::FRAMED_ERRORS.with_label_values(&[reason]).inc();
        tracing::warn!(
            transport_id = ?log_id,
            direction,
            error = %err,
            "framed transport error"
        );
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
        let encoded_payload = encode_message(&message);
        let namespace = "sync";
        let kind = match &message.payload {
            crate::transport::Payload::Text(_) => "text",
            crate::transport::Payload::Binary(_) => "binary",
        };
        let sequence = message.sequence;
        let frames = framed::encode_message(
            namespace,
            kind,
            sequence,
            &encoded_payload,
            &self.frame_config,
        )
        .map_err(|err| TransportError::Setup(format!("framing error: {err}")))?;
        tracing::debug!(
            transport_id = ?self.id,
            payload_len = encoded_payload.len(),
            sequence = message.sequence,
            frames = frames.len(),
            "queueing outbound framed message"
        );
        if frames.len() > 1 && !self.chunk_log_once.swap(true, Ordering::SeqCst) {
            tracing::info!(

                transport_id = ?self.id,
                sequence = message.sequence,
                payload_len = encoded_payload.len(),
                chunks = frames.len(),
                max_chunk_bytes = self.frame_config.chunk_size,
                "chunking outbound framed payload"
            );
        }

        for frame in frames {
            let mut bytes = frame.to_vec();
            if self.encryption.is_enabled() {
                bytes = self.encryption.encrypt(&bytes)?;
            }
            let priority = classify_priority(namespace, bytes.len());
            let queued = OutboundFrame {
                bytes,
                namespace: namespace.to_string(),
                priority,
                enqueued_at: Instant::now(),
            };
            let tx = match priority {
                OutboundPriority::High => &self.outbound_high_tx,
                OutboundPriority::Low => &self.outbound_low_tx,
            };
            tx.send(queued).map_err(|_| TransportError::ChannelClosed)?;
        }
        Ok(())
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        let sequence = self.next_seq("sync");
        self.send(TransportMessage::text(sequence, text.to_string()))?;
        Ok(sequence)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        let sequence = self.next_seq("sync");
        self.send(TransportMessage::binary(sequence, bytes.to_vec()))?;
        Ok(sequence)
    }

    fn send_namespaced(
        &self,
        namespace: &str,
        kind: &str,
        payload: &[u8],
    ) -> Result<u64, TransportError> {
        let sequence = self.next_seq(namespace);
        let frames = framed::encode_message(namespace, kind, sequence, payload, &self.frame_config)
            .map_err(|err| TransportError::Setup(format!("framing error: {err}")))?;
        tracing::debug!(
            transport_id = ?self.id,
            namespace,
            kind,
            payload_len = payload.len(),
            frames = frames.len(),
            sequence,
            "queueing outbound namespaced framed message"
        );
        for frame in frames {
            let mut bytes = frame.to_vec();
            if self.encryption.is_enabled() {
                bytes = self.encryption.encrypt(&bytes)?;
            }
            let priority = classify_priority(namespace, bytes.len());
            let queued = OutboundFrame {
                bytes,
                namespace: namespace.to_string(),
                priority,
                enqueued_at: Instant::now(),
            };
            let tx = match priority {
                OutboundPriority::High => &self.outbound_high_tx,
                OutboundPriority::Low => &self.outbound_low_tx,
            };
            tx.send(queued).map_err(|_| TransportError::ChannelClosed)?;
        }
        Ok(sequence)
    }

    fn recv(&self, timeout_duration: Duration) -> Result<TransportMessage, TransportError> {
        tracing::debug!(

            transport_id = ?self.id,
            timeout = ?timeout_duration,
            "waiting for inbound message"
        );
        let receiver = self.inbound_rx.lock().unwrap();
        let result = receiver.recv_timeout(timeout_duration);
        match result {
            Ok(message) => {
                tracing::debug!(

                    transport_id = ?self.id,
                    sequence = message.sequence,
                    payload = ?message.payload,
                    "received inbound message"
                );
                Ok(message)
            }
            Err(CrossbeamRecvTimeoutError::Timeout) => {
                tracing::debug!(

                    transport_id = ?self.id,
                    "recv timed out"
                );
                Err(TransportError::Timeout)
            }
            Err(CrossbeamRecvTimeoutError::Disconnected) => {
                tracing::warn!(

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
        pre_shared_key: Option<&[u8; 32]>,
    ) -> Result<RTCSessionDescription, TransportError> {
        session_description_from_payload(self, passphrase, pre_shared_key)
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
    session_id: String,
    poll_interval: Duration,
    passphrase: Option<String>,
    session_key: Arc<OnceCell<Arc<[u8; 32]>>>,
    accepted_tx: tokio_mpsc::UnboundedSender<OffererAcceptedTransport>,
    peer_tasks: AsyncMutex<HashMap<String, PeerNegotiatorHandle>>,
    peer_states: AsyncMutex<HashMap<String, PeerLifecycleState>>,
    max_negotiators: usize,
    controller_tracker: AsyncMutex<ControllerPeerTracker>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeerLifecycleState {
    Negotiating { controller: bool },
    Established { controller: bool },
}

impl PeerLifecycleState {
    fn is_controller(&self) -> bool {
        match self {
            PeerLifecycleState::Negotiating { controller }
            | PeerLifecycleState::Established { controller } => *controller,
        }
    }
}

#[derive(Default)]
pub(crate) struct ControllerPeerTracker {
    inflight: Option<String>,
    active: Option<String>,
}

impl ControllerPeerTracker {
    pub(crate) fn reserve(&mut self, peer_id: &str) -> bool {
        if self.inflight.is_some() || self.active.is_some() {
            return false;
        }
        self.inflight = Some(peer_id.to_string());
        true
    }

    pub(crate) fn promote(&mut self, peer_id: &str) {
        if self.inflight.as_deref() == Some(peer_id) {
            self.inflight = None;
            self.active = Some(peer_id.to_string());
        }
    }

    pub(crate) fn release(&mut self, peer_id: &str) {
        if self.inflight.as_deref() == Some(peer_id) {
            self.inflight = None;
        }
        if self.active.as_deref() == Some(peer_id) {
            self.active = None;
        }
    }
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
        metadata: Option<HashMap<String, String>>,
    ) -> Result<(Arc<Self>, OffererAcceptedTransport), TransportError> {
        let signaling_client = SignalingClient::connect(
            signaling_url,
            WebRtcRole::Offerer,
            passphrase,
            None,
            request_mcp_channel,
            metadata,
        )
        .await?;
        let client = Client::new();
        let signaling_base = signaling_url.trim_end_matches('/').to_string();
        let session_id = extract_session_id(signaling_url)?;
        let (accepted_tx, accepted_rx) = tokio_mpsc::unbounded_channel();

        let inner = Arc::new(OffererInner {
            client,
            signaling_client,
            signaling_base,
            session_id: session_id.clone(),
            poll_interval,
            passphrase: passphrase.map(|p| p.to_string()),
            session_key: Arc::new(OnceCell::new()),
            accepted_tx,
            peer_tasks: AsyncMutex::new(HashMap::new()),
            peer_states: AsyncMutex::new(HashMap::new()),
            max_negotiators: OFFERER_MAX_NEGOTIATORS,
            controller_tracker: AsyncMutex::new(ControllerPeerTracker::default()),
        });

        prime_session_key(
            &inner.session_key,
            inner.passphrase.as_deref(),
            &inner.session_id,
        );

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
    fn peer_label(peer: &PeerInfo) -> Option<String> {
        peer.metadata
            .as_ref()
            .and_then(|meta| meta.get("label").cloned())
            .filter(|value| !value.is_empty())
    }

    fn peer_is_controller(peer: &PeerInfo) -> bool {
        Self::peer_label(peer)
            .map(|label| {
                matches!(
                    label.as_str(),
                    CONTROLLER_CHANNEL_LABEL | LEGACY_CONTROLLER_CHANNEL_LABEL
                )
            })
            .unwrap_or(false)
    }

    fn peer_supports_secure_transport(peer: &PeerInfo) -> bool {
        let supports = Self::peer_label(peer)
            .map(|label| {
                matches!(
                    label.as_str(),
                    CONTROLLER_CHANNEL_LABEL
                        | LEGACY_CONTROLLER_CHANNEL_LABEL
                        | CONTROLLER_ACK_CHANNEL_LABEL
                        | CONTROLLER_STATE_CHANNEL_LABEL
                        | "private-beach-dashboard"
                        | "beach-manager"
                )
            })
            .unwrap_or(false);
        tracing::trace!(
            target = "beach::transport::webrtc",
            peer_id = %peer.id,
            supports,
            metadata = ?peer.metadata,
            "evaluated secure transport capability for peer"
        );
        supports
    }

    async fn reserve_controller_slot(&self, peer_id: &str) -> bool {
        let mut tracker = self.controller_tracker.lock().await;
        tracker.reserve(peer_id)
    }

    async fn promote_controller_slot(&self, peer_id: &str) {
        let mut tracker = self.controller_tracker.lock().await;
        tracker.promote(peer_id);
    }

    async fn release_controller_slot(&self, peer_id: &str) {
        let mut tracker = self.controller_tracker.lock().await;
        tracker.release(peer_id);
    }

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
        let peer_label = joined
            .peer
            .metadata
            .as_ref()
            .and_then(|meta| meta.get("label"))
            .map(|label| label.as_str())
            .unwrap_or("unknown");
        let secure_supported = Self::peer_supports_secure_transport(&joined.peer);
        tracing::info!(
            target = "beach::transport::webrtc",
            event = "peer_joined",
            peer_id = %joined.peer.id,
            role = ?joined.peer.role,
            label = peer_label,
            secure_supported,
            metadata = ?joined.peer.metadata,
            "offerer observed peer join"
        );
        if joined.peer.role != PeerRole::Client {
            tracing::debug!(

                peer_id = %joined.peer.id,
                role = ?joined.peer.role,
                "ignoring peer join for non-client role"
            );
            return;
        }

        let peer_id = joined.peer.id.clone();
        let controller_peer = Self::peer_is_controller(&joined.peer);
        let mut controller_reserved = false;
        if controller_peer {
            if !self.reserve_controller_slot(&peer_id).await {
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    peer_id = %peer_id,
                    "controller negotiation already active; throttling join"
                );
                return;
            }
            controller_reserved = true;
        }

        {
            let mut states = self.peer_states.lock().await;
            if let Some(state) = states.get(&joined.peer.id) {
                match state {
                    PeerLifecycleState::Negotiating { .. } => {
                        tracing::trace!(

                            peer_id = %joined.peer.id,
                            "peer negotiator already active; ignoring new join"
                        );
                        tracing::debug!(

                            peer_id = %joined.peer.id,
                            "peer negotiator already active"
                        );
                        if controller_reserved {
                            self.release_controller_slot(&peer_id).await;
                        }
                        return;
                    }
                    PeerLifecycleState::Established { .. } => {
                        tracing::trace!(

                            peer_id = %joined.peer.id,
                            "peer already has established transport; ignoring join"
                        );
                        tracing::debug!(

                            peer_id = %joined.peer.id,
                            "peer already has established transport"
                        );
                        if controller_reserved {
                            self.release_controller_slot(&peer_id).await;
                        }
                        return;
                    }
                }
            }
            states.insert(
                joined.peer.id.clone(),
                PeerLifecycleState::Negotiating {
                    controller: controller_peer,
                },
            );
            tracing::trace!(

                peer_id = %joined.peer.id,
                "peer lifecycle transitioned to negotiating"
            );
        }

        let mut tasks = self.peer_tasks.lock().await;
        if tasks.contains_key(&joined.peer.id) {
            tracing::debug!(

                peer_id = %joined.peer.id,
                "peer negotiator already active"
            );
            if controller_reserved {
                self.release_controller_slot(&peer_id).await;
            }
            return;
        }
        if tasks.len() >= self.max_negotiators {
            tracing::warn!(

                peer_id = %joined.peer.id,
                active = tasks.len(),
                max = self.max_negotiators,
                "dropping peer join due to negotiator capacity"
            );
            if controller_reserved {
                self.release_controller_slot(&peer_id).await;
            }
            return;
        }

        tracing::info!(

            peer_id = %joined.peer.id,
            "registering peer negotiator"
        );
        tracing::debug!(

            peer_id = %joined.peer.id,
            active_tasks = tasks.len(),
            "spawning negotiator task for joined peer"
        );

        let cancel = Arc::new(AtomicBool::new(false));
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

            peer_id = %peer_id,
            "handling peer left event"
        );
        let mut controller_peer = false;
        let mut tasks = self.peer_tasks.lock().await;
        if let Some(handle) = tasks.remove(peer_id) {
            tracing::debug!(

                peer_id = %peer_id,
                "setting cancel flag and aborting negotiator task for departed peer"
            );
            handle.cancel.store(true, Ordering::SeqCst);
            handle.task.abort();
        } else {
            tracing::debug!(

                peer_id = %peer_id,
                "no active negotiator task found for departed peer"
            );
        }

        let mut states = self.peer_states.lock().await;
        if let Some(state) = states.remove(peer_id) {
            controller_peer = state.is_controller();
        }
        drop(states);
        if controller_peer {
            self.release_controller_slot(peer_id).await;
        }
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

        let mut controller_promoted = false;
        let controller_peer = match result {
            Ok(Some(accepted)) => {
                let is_controller = {
                    let mut states = self.peer_states.lock().await;
                    let value = states
                        .get(peer_id)
                        .map(PeerLifecycleState::is_controller)
                        .unwrap_or(false);
                    states.insert(
                        peer_id.to_string(),
                        PeerLifecycleState::Established { controller: value },
                    );
                    tracing::trace!(

                        peer_id = %peer_id,
                        state = ?states.get(peer_id),
                        "peer lifecycle updated to established"
                    );
                    value
                };
                if is_controller {
                    self.promote_controller_slot(peer_id).await;
                    controller_promoted = true;
                }
                if self.accepted_tx.send(accepted).is_err() {
                    tracing::debug!(

                        peer_id = %peer_id,
                        "dropping accepted transport because receiver closed"
                    );
                }
                is_controller
            }
            Ok(None) => {
                let is_controller = {
                    let mut states = self.peer_states.lock().await;
                    let value = states
                        .get(peer_id)
                        .map(PeerLifecycleState::is_controller)
                        .unwrap_or(false);
                    states.remove(peer_id);
                    tracing::trace!(

                        peer_id = %peer_id,
                        "peer lifecycle entry removed after negotiator concluded without transport"
                    );
                    value
                };
                tracing::debug!(

                    peer_id = %peer_id,
                    cancelled = cancel_flag.load(Ordering::SeqCst),
                    "peer negotiation concluded without establishing transport"
                );
                is_controller
            }
            Err(err) => {
                let is_controller = {
                    let mut states = self.peer_states.lock().await;
                    let value = states
                        .get(peer_id)
                        .map(PeerLifecycleState::is_controller)
                        .unwrap_or(false);
                    states.remove(peer_id);
                    tracing::trace!(

                        peer_id = %peer_id,
                        "peer lifecycle entry removed after negotiator error"
                    );
                    value
                };
                tracing::warn!(

                    peer_id = %peer_id,
                    cancelled = cancel_flag.load(Ordering::SeqCst),
                    error = %err,
                    "peer negotiation ended with error"
                );
                is_controller
            }
        };

        if controller_peer && !controller_promoted {
            self.release_controller_slot(peer_id).await;
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
    let peer_label = OffererInner::peer_label(&peer);
    let signaling_base = inner.signaling_base.clone();

    let mut setting = SettingEngine::default();
    // Force IPv4 so Dockerized managers do not attempt unreachable udp6 STUN
    // candidates (they produce noisy logs and stall ICE when the bridge lacks v6).
    setting.set_network_types(vec![NetworkType::Udp4]);
    apply_nat_hint(&mut setting);
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    let disable_stun = std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_ok();
    let mut config = RTCConfiguration::default();
    let selection = match load_turn_ice_servers().await {
        Ok(selection) => selection,
        Err(AuthError::TurnNotEntitled) => {
            return Err(TransportError::Setup(
                "TURN transport requires pb:transport.turn entitlement".into(),
            ));
        }
        Err(err) => {
            tracing::debug!(
                target = "beach::transport::webrtc",
                error = %err,
                "invalid ICE override; falling back to host candidates"
            );
            IceServerSelection::with_source(IceServerSource::HostOnly, None)
        }
    };

    let appended_stun = match selection.servers.as_ref() {
        Some(servers) => {
            let mut combined = servers.clone();
            if !disable_stun {
                combined.push(default_stun_server());
            }
            config.ice_servers = combined;
            !disable_stun
        }
        None => {
            if !disable_stun {
                config.ice_servers = vec![default_stun_server()];
                true
            } else {
                false
            }
        }
    };
    log_ice_configuration("offerer", &selection, appended_stun);

    let pc = Arc::new(
        api.new_peer_connection(config)
            .await
            .map_err(to_setup_error)?,
    );
    let channels = WebRtcChannels::new();
    let pending_ice = Arc::new(AsyncMutex::new(Vec::new()));
    let handshake_id = Uuid::new_v4().to_string();
    let handshake_span = tracing::trace_span!(
        target: "webrtc",
        "webrtc_handshake",
        role = "offerer",
        handshake_id = %handshake_id,
        remote_peer = %peer.id,
        thread = %current_thread_label()
    );
    let _handshake_span_guard = handshake_span.enter();
    let handshake_id_arc = Arc::new(handshake_id.clone());
    let pre_shared_key_cell = Arc::new(OnceCell::<Arc<[u8; 32]>>::new());
    prime_pre_shared_key(
        &pre_shared_key_cell,
        &inner.session_key,
        inner.passphrase.as_deref(),
        inner.session_id.as_str(),
        &handshake_id,
    );
    let peer_id = peer.id.clone();
    let peer_id_for_candidates = peer_id.clone();

    let signaling_for_candidates = Arc::clone(&inner.signaling_client);
    let cancel_for_candidates = Arc::clone(&cancel_flag);
    let handshake_for_candidates = handshake_id_arc.clone();
    let pre_shared_key_for_candidates = Arc::clone(&pre_shared_key_cell);
    let session_key_for_candidates = Arc::clone(&inner.session_key);
    let session_id_for_candidates = inner.session_id.clone();
    let passphrase_for_candidates = inner.passphrase.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        let peer_id = peer_id_for_candidates.clone();
        let cancel_flag = Arc::clone(&cancel_for_candidates);
        let handshake_id = handshake_for_candidates.clone();
        let pre_shared_key_cell = Arc::clone(&pre_shared_key_for_candidates);
        let session_key_cell = Arc::clone(&session_key_for_candidates);
        let session_id = session_id_for_candidates.clone();
        let passphrase = passphrase_for_candidates.clone();
        Box::pin(async move {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            if let Some(cand) = candidate {
                let handshake_key = match await_pre_shared_key(
                    &pre_shared_key_cell,
                    &session_key_cell,
                    passphrase.as_deref(),
                    session_id.as_str(),
                    handshake_id.as_str(),
                )
                .await
                {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::warn!(
                            target = "webrtc",
                            handshake_id = %handshake_id.as_str(),
                            peer_id = %peer_id,
                            error = %err,
                            "failed to derive handshake key before sending local ice candidate"
                        );
                        None
                    }
                };
                if let Err(err) = signaling
                    .send_ice_candidate_to_peer(
                        cand,
                        handshake_id.as_str(),
                        &peer_id,
                        handshake_key,
                    )
                    .await
                {
                    tracing::warn!(

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
    let session_key_for_signals = Arc::clone(&inner.session_key);
    let session_id_for_signals = inner.session_id.clone();
    let pre_shared_key_for_signals = Arc::clone(&pre_shared_key_cell);
    let ice_task = spawn_on_global({
        let pre_shared_key_for_signals = Arc::clone(&pre_shared_key_for_signals);
        let session_key_for_signals = Arc::clone(&session_key_for_signals);
        let session_id_for_signals = session_id_for_signals.clone();
        async move {
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
                    let derived_key = match await_pre_shared_key(
                        &pre_shared_key_for_signals,
                        &session_key_for_signals,
                        passphrase_for_signals.as_deref(),
                        session_id_for_signals.as_str(),
                        handshake_for_incoming.as_str(),
                    )
                    .await
                    {
                        Ok(key) => key,
                        Err(err) => {
                            tracing::warn!(

                                peer_id = %peer_id_for_incoming,
                                error = %err,
                                "failed to derive handshake key for remote ice candidate"
                            );
                            continue;
                        }
                    };
                    let resolved = match resolve_ice_candidate(
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        sealed,
                        passphrase_for_signals.as_deref(),
                        derived_key.as_ref().map(|key| key.as_ref()),
                        handshake_for_incoming.as_str(),
                        &peer_id_for_incoming,
                        local_peer_id.as_str(),
                    ) {
                        Ok(resolved) => resolved,
                        Err(err) => {
                            tracing::warn!(

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
                    if let Some(meta) = parse_candidate_info(&init.candidate) {
                        tracing::debug!(
                            target = "beach::transport::webrtc",
                            event = "remote_candidate_received",
                            role = "offerer",
                            handshake_id = %handshake_for_incoming,
                            session_id = %session_id_for_signals,
                            peer_id = %peer_id_for_incoming,
                            ip = %meta.ip,
                            port = meta.port,
                            scope = meta.scope,
                            "decoded remote ICE candidate"
                        );
                        if meta.scope == "loopback" {
                            tracing::warn!(
                                target = "beach::transport::webrtc",
                                event = "remote_candidate_loopback",
                                role = "offerer",
                                handshake_id = %handshake_for_incoming,
                                session_id = %session_id_for_signals,
                                peer_id = %peer_id_for_incoming,
                                ip = %meta.ip,
                                port = meta.port,
                                "remote ICE candidate uses loopback address; likely unreachable"
                            );
                        }
                    }
                    let has_remote = pc_for_incoming.remote_description().await.is_some();
                    if !has_remote {
                        let mut queue = pending_for_incoming.lock().await;
                        queue.push(init);
                        continue;
                    }
                    if let Err(err) = pc_for_incoming.add_ice_candidate(init.clone()).await {
                        tracing::warn!(

                            peer_id = %peer_id_for_incoming,
                            error = %err,
                            "failed to add remote ice candidate"
                        );
                        let mut queue = pending_for_incoming.lock().await;
                        queue.push(init);
                    }
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
    let peer_label_for_logging = peer_label
        .as_ref()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let dc = pc
        .create_data_channel("beach", Some(dc_init))
        .await
        .map_err(to_setup_error)?;
    let peer_id_for_primary_open = peer.id.clone();
    let handshake_for_primary_open = handshake_id.clone();
    let label_for_primary_open = peer_label_for_logging.clone();
    dc.on_open(Box::new(move || {
        let notify = dc_open_notify.clone();
        let peer_id = peer_id_for_primary_open.clone();
        let handshake_for_log = handshake_for_primary_open.clone();
        let label_for_log = label_for_primary_open.clone();
        Box::pin(async move {
            tracing::info!(
                peer_id = %peer_id,
                %handshake_for_log,
                label = %label_for_log,
                "primary data channel opened"
            );
            notify.notify_waiters();
            notify.notify_one();
        })
    }));
    let peer_id_for_close = peer.id.clone();
    let handshake_for_close = handshake_id.clone();
    let label_for_close = peer_label_for_logging.clone();
    dc.on_close(Box::new(move || {
        let peer_id = peer_id_for_close.clone();
        let handshake = handshake_for_close.clone();
        let label = label_for_close.clone();
        Box::pin(async move {
            tracing::info!(
                peer_id = %peer_id,
                %handshake,
                label = %label,
                "primary data channel closed"
            );
        })
    }));

    let secure_transport_active = secure_transport_enabled()
        && OffererInner::peer_supports_secure_transport(&peer)
        && inner
            .passphrase
            .as_ref()
            .map(|p| !p.trim().is_empty())
            .unwrap_or(false);
    tracing::info!(
        role = "offerer",
        secure_transport_active,
        has_passphrase = inner.passphrase.is_some(),
        env_enabled = secure_transport_enabled(),
        "offerer: checking if secure transport should be enabled"
    );
    let handshake_dc = if secure_transport_active {
        tracing::info!(
            role = "offerer",
            label = HANDSHAKE_CHANNEL_LABEL,
            "offerer: creating handshake data channel"
        );
        let channel = pc
            .create_data_channel(HANDSHAKE_CHANNEL_LABEL, Some(handshake_channel_init()))
            .await
            .map_err(to_setup_error)?;
        let inbox = Arc::new(HandshakeInbox::new());
        let inbox_for_dc = Arc::clone(&inbox);
        channel.on_message(Box::new(move |msg: DataChannelMessage| {
            let inbox = Arc::clone(&inbox_for_dc);
            Box::pin(async move {
                tracing::trace!(
                    target = "webrtc",
                    event = "handshake_channel_inbound_raw",
                    bytes = msg.data.len(),
                    preview = %hex_preview(&msg.data),
                    "secure handshake received raw datachannel message (offerer)"
                );
                inbox.push(msg.data.to_vec()).await;
            })
        }));
        let peer_id_for_handshake_open = peer.id.clone();
        let handshake_for_handshake_open = handshake_id.clone();
        channel.on_open(Box::new(move || {
            let peer_id = peer_id_for_handshake_open.clone();
            let handshake = handshake_for_handshake_open.clone();
            Box::pin(async move {
                tracing::info!(
                    peer_id = %peer_id,
                    %handshake,
                    label = HANDSHAKE_CHANNEL_LABEL,
                    "handshake data channel opened"
                );
            })
        }));
        let peer_id_for_handshake_close = peer.id.clone();
        let handshake_for_handshake_close = handshake_id.clone();
        channel.on_close(Box::new(move || {
            let peer_id = peer_id_for_handshake_close.clone();
            let handshake = handshake_for_handshake_close.clone();
            Box::pin(async move {
                tracing::info!(
                    peer_id = %peer_id,
                    %handshake,
                    label = HANDSHAKE_CHANNEL_LABEL,
                    "handshake data channel closed"
                );
            })
        }));
        Some((channel, inbox))
    } else {
        tracing::info!(
            role = "offerer",
            "offerer: NOT creating handshake channel (secure transport disabled)"
        );
        None
    };
    if let Some((ref dc, _)) = handshake_dc {
        tracing::info!(

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
    tracing::info!(
        target = "beach::transport::webrtc",
        role = "offerer",
        handshake_id = %handshake_id,
        sdp_len = local_desc.sdp.len(),
        "offer created"
    );
    let offerer_peer_id = inner
        .signaling_client
        .assigned_peer_id()
        .await
        .unwrap_or_else(|| inner.signaling_client.peer_id().to_string());

    let shared_key = await_pre_shared_key(
        &pre_shared_key_cell,
        &inner.session_key,
        inner.passphrase.as_deref(),
        inner.session_id.as_str(),
        &handshake_id,
    )
    .await?;
    let payload = payload_from_description(
        &local_desc,
        &handshake_id,
        &offerer_peer_id,
        &peer.id,
        inner.passphrase.as_deref(),
        shared_key.as_ref().map(|key| key.as_ref()),
    )?;

    if cancel_flag.load(Ordering::SeqCst) {
        let _ = pc.close().await;
        ice_task.abort();
        return Ok(None);
    }

    post_sdp(&inner.client, &signaling_base, "offer", &[], &payload).await?;
    tracing::info!(
        target = "beach::transport::webrtc",
        role = "offerer",
        handshake_id = %handshake_id,
        remote_peer = %peer.id,
        "offer posted to signaling"
    );

    let answer = poll_answer_for_peer(
        &inner.client,
        &signaling_base,
        inner.poll_interval,
        &handshake_id,
    )
    .await?;

    let remote_desc = answer.to_session_description(
        inner.passphrase.as_deref(),
        shared_key.as_ref().map(|key| key.as_ref()),
    )?;
    pc.set_remote_description(remote_desc)
        .await
        .map_err(to_setup_error)?;

    {
        let mut queued = pending_ice.lock().await;
        for candidate in queued.drain(..) {
            if let Err(err) = pc.add_ice_candidate(candidate.clone()).await {
                tracing::warn!(

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

        peer_id = %peer.id,
        %handshake_id,
        label = %peer_label_for_logging,
        "waiting for datachannel to open (15s timeout)"
    );
    if tokio::time::timeout(Duration::from_secs(15), dc_notify.notified())
        .await
        .is_err()
    {
        tracing::warn!(
            %handshake_id,
            label = %peer_label_for_logging,
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
    install_peer_connection_tracing(
        &pc,
        "offerer",
        Some(handshake_id.clone()),
        Some(inner.session_id.clone()),
        Some(peer.id.clone()),
        Some(local_id),
    );
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
        false,
    ));
    let transport_dyn: Arc<dyn Transport> = transport.clone();
    let publish_label = peer_label.unwrap_or_else(|| "beach".to_string());

    channels.publish(publish_label, transport_dyn.clone());

    // Run secure handshake BEFORE waiting for __ready__ sentinel
    // The answerer will send __ready__ only after completing the handshake,
    // so the offerer must initiate the handshake first
    tracing::info!(

        role = "offerer",
        peer_id = %peer.id,
        secure_transport_active,
        has_passphrase = inner.passphrase.is_some(),
        has_handshake_channel = handshake_dc.is_some(),
        "offerer: evaluating whether to run secure handshake"
    );
    let secure_context = if let (true, Some(passphrase), Some((handshake_channel, inbox))) = (
        secure_transport_active,
        inner.passphrase.as_ref(),
        handshake_dc.clone(),
    ) {
        tracing::info!(

            role = "offerer",
            handshake_id = %handshake_id,
            offerer_peer = %offerer_peer_id,
            remote_peer = %peer_id,
            channel_state = ?handshake_channel.ready_state(),
            "offerer: about to run handshake as Initiator"
        );
        tracing::trace!(

            role = "offerer",
            handshake_id = %handshake_id,
            event = "pre_shared_key_wait",
            thread = %current_thread_label()
        );
        let handshake_key = await_pre_shared_key(
            &pre_shared_key_cell,
            &inner.session_key,
            Some(passphrase.as_str()),
            inner.session_id.as_str(),
            &handshake_id,
        )
        .await?
        .ok_or_else(|| {
            TransportError::Setup("handshake key unavailable for secure transport".into())
        })?;
        tracing::trace!(

            role = "offerer",
            handshake_id = %handshake_id,
            event = "pre_shared_key_acquired",
            thread = %current_thread_label()
        );
        tracing::debug!(

            role = "offerer",
            handshake_id = %handshake_id,
            key_path = "handshake_for_noise",
            handshake_hash = %truncated_key_hash(handshake_key.as_ref()),
            "acquired handshake key for Noise handshake"
        );
        let prologue_context = build_prologue_context(&handshake_id, &offerer_peer_id, &peer_id);
        let params = HandshakeParams {
            handshake_key: handshake_key.clone(),
            handshake_id: handshake_id.clone(),
            local_peer_id: offerer_peer_id.clone(),
            remote_peer_id: peer_id.clone(),
            prologue_context,
            inbox,
        };
        tracing::debug!(

            role = "offerer",
            handshake_id = %handshake_id,
            "offerer: calling run_handshake as Initiator"
        );
        let result = run_handshake(HandshakeRole::Initiator, handshake_channel, params).await?;
        tracing::info!(

            role = "offerer",
            handshake_id = %handshake_id,
            verification = %result.verification_code,
            "offerer: handshake completed successfully as Initiator"
        );
        if let Some(delay) = offer_encryption_delay() {
            tracing::debug!(
                role = "offerer",
                handshake_id = %handshake_id,
                delay_ms = delay.as_millis(),
                "development encryption delay active before enabling transport"
            );
            sleep(delay).await;
        }
        transport.enable_encryption(&result)?;
        Some(Arc::new(result))
    } else {
        tracing::info!(

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

        peer_id = %peer.id,
        transport_id = ?local_id,
        dc_state = ?dc_state_before,
        pc_state = ?pc_state_before,
        "starting readiness handshake"
    );

    // Give the data channel on_message callback a chance to fire and enqueue
    // any messages that arrived immediately when the channel opened
    tracing::trace!(

        peer_id = %peer.id,
        "sleeping 10ms before polling for __ready__"
    );
    sleep(Duration::from_millis(10)).await;
    let dc_state_after = transport._dc.ready_state();
    let pc_state_after = pc.connection_state();
    tracing::trace!(

        peer_id = %peer.id,
        dc_state_after_sleep = ?dc_state_after,
        pc_state_after_sleep = ?pc_state_after,
        "sleep completed"
    );

    tracing::debug!(

        peer_id = %peer.id,
        "beginning to poll for __ready__ sentinel"
    );

    let mut ready_seen = false;
    let mut readiness_attempts = 0usize;
    for attempt in 0..READY_ACK_POLL_ATTEMPTS {
        readiness_attempts = attempt + 1;
        if cancel_flag.load(Ordering::SeqCst) {
            tracing::warn!(

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

                    peer_id = %peer.id,
                    attempt = attempt,
                    payload = ?message.payload,
                    payload_text = ?message.payload.as_text(),
                    "offerer received message during readiness handshake"
                );
                if message.payload.as_text() == Some("__ready__") {
                    ready_seen = true;
                    tracing::info!(

                        peer_id = %peer.id,
                        attempt = attempt,
                        "received __ready__ sentinel from answerer"
                    );
                    break;
                } else {
                    tracing::debug!(

                        peer_id = %peer.id,
                        payload = ?message.payload,
                        "received unexpected message during readiness handshake"
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(

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

        peer_id = %peer.id,
        attempts = readiness_attempts,
        ready_seen = ready_seen,
        "readiness handshake polling finished"
    );

    if !ready_seen {
        tracing::warn!(

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

        peer_id = %peer.id,
        "sending __offer_ready__ sentinel to answerer"
    );

    tracing::debug!(

        peer_id = %peer.id,
        handshake_id = %handshake_id,
        attempts = readiness_attempts,
        "readiness handshake completed"
    );
    handshake_complete.store(true, Ordering::SeqCst);

    if let Err(err) = transport.send_text("__offer_ready__") {
        tracing::warn!(

            peer_id = %peer.id,
            error = %err,
            "failed to send offer ready sentinel"
        );
    }

    tracing::info!(

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
            false,
        ));
        let mcp_transport_dyn: Arc<dyn Transport> = mcp_transport;
        channels.publish(MCP_CHANNEL_LABEL.to_string(), mcp_transport_dyn);
        tracing::info!(

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

    let connection_metadata = peer_metadata.clone();
    Ok(Some(OffererAcceptedTransport {
        peer_id: peer_id,
        handshake_id,
        metadata: peer_metadata,
        connection: WebRtcConnection::new(
            transport_dyn,
            channels,
            secure_context,
            Some(Arc::clone(&inner.signaling_client)),
            Some(connection_metadata),
        ),
    }))
}

pub async fn connect_via_signaling(
    signaling_url: &str,
    role: WebRtcRole,
    poll_interval: Duration,
    passphrase: Option<&str>,
    label: Option<&str>,
    request_mcp_channel: bool,
    metadata: Option<HashMap<String, String>>,
) -> Result<WebRtcConnection, TransportError> {
    match role {
        WebRtcRole::Offerer => {
            let (_supervisor, accepted) = OffererSupervisor::connect(
                signaling_url,
                poll_interval,
                passphrase,
                request_mcp_channel,
                metadata,
            )
            .await?;
            Ok(accepted.connection)
        }
        WebRtcRole::Answerer => {
            connect_answerer(signaling_url, poll_interval, passphrase, label, metadata).await
        }
    }
}

/// Best-effort warmup for session key derivation so later handshakes don’t pay the KDF cost.
pub async fn warm_session_key(passphrase: Option<&str>, session_id: &str) {
    let cell = Arc::new(OnceCell::<Arc<[u8; 32]>>::new());
    if let Err(err) = ensure_session_key(&cell, passphrase, session_id).await {
        tracing::warn!(
            target = "beach::transport::webrtc",
            session_id = %session_id,
            error = %err,
            "session key warmup failed"
        );
    } else {
        tracing::trace!(
            target = "beach::transport::webrtc",
            session_id = %session_id,
            "session key warmup complete"
        );
    }
}

async fn connect_answerer(
    signaling_url: &str,
    poll_interval: Duration,
    passphrase: Option<&str>,
    label: Option<&str>,
    metadata: Option<HashMap<String, String>>,
) -> Result<WebRtcConnection, TransportError> {
    let started = std::time::Instant::now();
    let passphrase_owned = passphrase.map(|s| s.to_string());
    let session_id = extract_session_id(signaling_url)?;
    let session_key_cell = Arc::new(OnceCell::<Arc<[u8; 32]>>::new());
    prime_session_key(
        &session_key_cell,
        passphrase_owned.as_deref(),
        session_id.as_str(),
    );
    if let Err(err) = ensure_session_key(
        &session_key_cell,
        passphrase_owned.as_deref(),
        session_id.as_str(),
    )
    .await
    {
        tracing::warn!(

            role = "answerer",
            session_id = %session_id,
            error = %err,
            "eager session key derivation failed"
        );
    } else {
        tracing::debug!(
            target = "beach::transport::webrtc",
            role = "answerer",
            session_id = %session_id,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "session key derived eagerly"
        );
    }
    let secure_transport_active = secure_transport_enabled()
        && passphrase_owned
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    let (secure_tx, secure_rx) =
        oneshot::channel::<Result<Option<Arc<HandshakeResult>>, TransportError>>();
    let secure_sender = Arc::new(AsyncMutex::new(Some(secure_tx)));
    if !secure_transport_active {
        tracing::trace!(

            role = "answerer",
            event = "secure_sender_lock_wait",
            reason = "secure_transport_disabled",
            thread = %current_thread_label()
        );
        let mut sender_guard = secure_sender.lock().await;
        tracing::trace!(

            role = "answerer",
            event = "secure_sender_lock_acquired",
            reason = "secure_transport_disabled",
            thread = %current_thread_label()
        );
        if let Some(sender) = sender_guard.take() {
            let _ = sender.send(Ok(None));
        }
    }
    let client = Client::new();
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        url = %signaling_url,
        "connecting signaling client"
    );
    let signaling_client = SignalingClient::connect(
        signaling_url,
        WebRtcRole::Answerer,
        passphrase,
        label.map(|s| s.to_string()),
        false,
        metadata,
    )
    .await?;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        "signaling client connected"
    );
    let (expected_remote_peer, _) = signaling_client
        .wait_for_remote_peer_with_generation()
        .await?;
    tracing::debug!(
        target = "beach::transport::webrtc",
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
            target = "beach::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "start"
        );
        let peer_param = [("peer_id", assigned_peer_id.as_str())];
        let fetch_attempt = fetch_sdp(&client, signaling_url, "offer", &peer_param).await;
        tracing::debug!(
            target = "beach::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "end",
            result = ?fetch_attempt
        );
        match fetch_attempt? {
            Some(payload) => {
                if payload.from_peer != expected_remote_peer {
                    tracing::warn!(
                        target = "beach::transport::webrtc",
                        role = "answerer",
                        expected_remote = %expected_remote_peer,
                        received_remote = %payload.from_peer,
                        "ignoring offer from unexpected peer"
                    );
                    continue;
                }
                tracing::info!(
                    target = "beach::transport::webrtc",
                    role = "answerer",
                    handshake_id = %payload.handshake_id,
                    remote_peer = %payload.from_peer,
                    "accepted offer"
                );
                break payload;
            }
            None => {
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "start",
                    poll_ms = poll_interval.as_millis() as u64
                );
                sleep(poll_interval).await;
                tracing::debug!(
                    target = "beach::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "end"
                );
            }
        }
    };
    let pre_shared_key_cell = Arc::new(OnceCell::<Arc<[u8; 32]>>::new());
    prime_pre_shared_key(
        &pre_shared_key_cell,
        &session_key_cell,
        passphrase_owned.as_deref(),
        session_id.as_str(),
        &offer_payload.handshake_id,
    );
    let shared_key = await_pre_shared_key(
        &pre_shared_key_cell,
        &session_key_cell,
        passphrase_owned.as_deref(),
        session_id.as_str(),
        &offer_payload.handshake_id,
    )
    .await?;
    let offer_desc = session_description_from_payload(
        &offer_payload,
        passphrase_owned.as_deref(),
        shared_key.as_ref().map(|key| key.as_ref()),
    )?;
    let remote_offer_peer = offer_payload.from_peer.clone();
    let handshake_id = Arc::new(offer_payload.handshake_id.clone());
    let answerer_span = tracing::trace_span!(
        target: "webrtc",
        "webrtc_handshake",
        role = "answerer",
        handshake_id = %handshake_id.as_str(),
        remote_peer = %remote_offer_peer,
        thread = %current_thread_label()
    );
    let _answerer_span_guard = answerer_span.enter();

    let mut setting = SettingEngine::default();
    // Keep the answerer on IPv4-only transports for the same reason as the offerer:
    // IPv6 candidates from Docker are unroutable and make STUN gathering hang.
    setting.set_network_types(vec![NetworkType::Udp4]);
    apply_nat_hint(&mut setting);
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    // Add a public STUN server so we gather server-reflexive candidates.
    // Without this, host-only candidates can fail in common NAT setups where the
    // browser uses mDNS/srflx and the offerer has no reflexive candidates.
    let disable_stun = std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_ok();
    let mut config = RTCConfiguration::default();
    let selection = match load_turn_ice_servers().await {
        Ok(selection) => selection,
        Err(AuthError::TurnNotEntitled) => {
            return Err(TransportError::Setup(
                "TURN transport requires pb:transport.turn entitlement".into(),
            ));
        }
        Err(err) => {
            tracing::debug!(
                target = "beach::transport::webrtc",
                error = %err,
                "invalid ICE override; falling back to host candidates"
            );
            IceServerSelection::with_source(IceServerSource::HostOnly, None)
        }
    };

    let appended_stun = match selection.servers.as_ref() {
        Some(servers) => {
            let mut combined = servers.clone();
            if !disable_stun {
                combined.push(default_stun_server());
            }
            config.ice_servers = combined;
            !disable_stun
        }
        None => {
            if !disable_stun {
                config.ice_servers = vec![default_stun_server()];
                true
            } else {
                false
            }
        }
    };
    log_ice_configuration("answerer", &selection, appended_stun);

    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "start"
    );
    let pc_result = api.new_peer_connection(config).await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "end",
        result = ?pc_result
    );
    let pc = Arc::new(pc_result.map_err(to_setup_error)?);
    let channels = WebRtcChannels::new();

    let signaling_for_candidates = Arc::clone(&signaling_client);
    let handshake_for_candidates = Arc::clone(&handshake_id);
    let pre_shared_key_for_candidates = Arc::clone(&pre_shared_key_cell);
    let session_key_for_candidates = Arc::clone(&session_key_cell);
    let session_id_for_candidates = session_id.clone();
    let passphrase_for_candidates = passphrase_owned.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        let handshake_id = Arc::clone(&handshake_for_candidates);
        let pre_shared_key_cell = Arc::clone(&pre_shared_key_for_candidates);
        let session_key_cell = Arc::clone(&session_key_for_candidates);
        let session_id = session_id_for_candidates.clone();
        let passphrase = passphrase_for_candidates.clone();
        Box::pin(async move {
            if let Some(cand) = candidate {
                tracing::debug!(

                    role = "offerer",
                    candidate = %cand.to_string(),
                    "local ice candidate gathered"
                );
                let handshake_key = match await_pre_shared_key(
                    &pre_shared_key_cell,
                    &session_key_cell,
                    passphrase.as_deref(),
                    session_id.as_str(),
                    handshake_id.as_str(),
                )
                .await
                {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::warn!(
                            handshake_id = %handshake_id.as_str(),
                            error = %err,
                            "answerer failed to derive handshake key before sending local ice candidate"
                        );
                        None
                    }
                };
                if let Err(err) = signaling
                    .send_ice_candidate(cand, handshake_id.as_str(), handshake_key)
                    .await
                {
                    tracing::warn!(

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
    let pre_shared_key_for_signals = Arc::clone(&pre_shared_key_cell);
    let session_key_for_signals = Arc::clone(&session_key_cell);
    let session_id_for_signals = session_id.clone();
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
                        handshake_id,
                        "answerer ignoring remote ICE candidate for stale handshake"
                    );
                    continue;
                }
                let derived_key = match await_pre_shared_key(
                    &pre_shared_key_for_signals,
                    &session_key_for_signals,
                    passphrase_for_signals.as_deref(),
                    session_id_for_signals.as_str(),
                    handshake_for_incoming.as_str(),
                )
                .await
                {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::warn!(

                            handshake_id,
                            error = %err,
                            "answerer failed to derive handshake key for remote ice candidate"
                        );
                        continue;
                    }
                };
                let resolved = match resolve_ice_candidate(
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                    sealed,
                    passphrase_for_signals.as_deref(),
                    derived_key.as_ref().map(|key| key.as_ref()),
                    handshake_for_incoming.as_str(),
                    &remote_offer_peer_for_signals,
                    assigned_peer_id_for_signals.as_str(),
                ) {
                    Ok(resolved) => resolved,
                    Err(err) => {
                        tracing::warn!(

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
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_for_incoming.as_str(),
                        event = "pending_ice_queue_lock_wait",
                        reason = "queue_remote",
                        thread = %current_thread_label()
                    );
                    let mut queue = pending_for_incoming.lock().await;
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_for_incoming.as_str(),
                        event = "pending_ice_queue_lock_acquired",
                        thread = %current_thread_label(),
                        queue_len = queue.len()
                    );
                    queue.push(init);
                    tracing::debug!(
                        role = "answerer",
                        "queued remote ice candidate (remote description not set yet)"
                    );
                    continue;
                }
                if let Err(err) = pc_for_incoming.add_ice_candidate(init.clone()).await {
                    tracing::warn!(

                        error = %err,
                        "answerer failed to add remote ice candidate"
                    );
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_for_incoming.as_str(),
                        event = "pending_ice_queue_lock_wait",
                        reason = "fallback_queue",
                        thread = %current_thread_label()
                    );
                    let mut queue = pending_for_incoming.lock().await;
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_for_incoming.as_str(),
                        event = "pending_ice_queue_lock_acquired",
                        reason = "fallback_queue",
                        thread = %current_thread_label(),
                        queue_len = queue.len()
                    );
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
    tracing::debug!(?client_id, ?peer_id, "answerer allocating transport ids");
    install_peer_connection_tracing(
        &pc,
        "answerer",
        Some(handshake_id.as_ref().clone()),
        Some(session_id.clone()),
        Some(remote_offer_peer.clone()),
        Some(client_id),
    );
    let signaling_for_dc = Arc::clone(&signaling_client);
    let channels_registry = channels.clone();
    let secure_sender_holder = Arc::clone(&secure_sender);
    let passphrase_for_secure = passphrase_owned.clone();
    let assigned_peer_for_secure = assigned_peer_id.clone();
    let remote_peer_for_secure = remote_offer_peer.clone();
    let handshake_for_secure = Arc::clone(&handshake_id);
    let session_id_for_secure = session_id.clone();
    let session_key_for_secure = Arc::clone(&session_key_cell);
    let pre_shared_key_for_secure = Arc::clone(&pre_shared_key_cell);
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
        let session_id_value = session_id_for_secure.clone();
        let session_key_cell_value = Arc::clone(&session_key_for_secure);
        let pre_shared_key_cell_value = Arc::clone(&pre_shared_key_for_secure);
        let label = dc.label().to_string();
        let raw_mode = label == CONTROLLER_CHANNEL_LABEL;
        let passphrase_setup = passphrase_value.clone();
        let preregistered_inbox = if label == HANDSHAKE_CHANNEL_LABEL
            && secure_transport_active
            && passphrase_setup
                .as_ref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        {
            let inbox = Arc::new(HandshakeInbox::new());
            let inbox_for_dc = Arc::clone(&inbox);
            dc.on_message(Box::new(move |msg: DataChannelMessage| {
                let inbox = Arc::clone(&inbox_for_dc);
                Box::pin(async move {
                    tracing::trace!(
                        target = "webrtc",
                        event = "handshake_channel_inbound_raw",
                        bytes = msg.data.len(),
                        preview = %hex_preview(&msg.data),
                        "secure handshake received raw datachannel message (answerer)"
                    );
                    inbox.push(msg.data.to_vec()).await;
                })
            }));
            Some(inbox)
        } else {
            None
        };
        Box::pin(async move {
            let mut preregistered_inbox = preregistered_inbox;
            let label = label;
            tracing::debug!(

                role = "answerer",
                label = %label,
                "incoming data channel announced"
            );

            if label == HANDSHAKE_CHANNEL_LABEL {
                if !secure_transport_active {
                    tracing::trace!(

                        role = "answerer",
                        event = "secure_sender_lock_wait",
                        reason = "secure_transport_disabled",
                        thread = %current_thread_label()
                    );
                    let mut sender_guard = secure_sender.lock().await;
                    tracing::trace!(

                        role = "answerer",
                        event = "secure_sender_lock_acquired",
                        reason = "secure_transport_disabled",
                        thread = %current_thread_label()
                    );
                    if let Some(sender) = sender_guard.take() {
                        let _ = sender.send(Ok(None));
                    }
                    return;
                }
                let Some(passphrase) = passphrase_value.clone() else {
                    tracing::trace!(

                        role = "answerer",
                        event = "secure_sender_lock_wait",
                        reason = "missing_passphrase",
                        thread = %current_thread_label()
                    );
                    let mut sender_guard = secure_sender.lock().await;
                    tracing::trace!(

                        role = "answerer",
                        event = "secure_sender_lock_acquired",
                        reason = "missing_passphrase",
                        thread = %current_thread_label()
                    );
                    if let Some(sender) = sender_guard.take() {
                        let _ = sender.send(Ok(None));
                    }
                    return;
                };
                let inbox = if let Some(inbox) = preregistered_inbox.take() {
                    inbox
                } else {
                    let inbox = Arc::new(HandshakeInbox::new());
                    let inbox_for_dc = Arc::clone(&inbox);
                    dc.on_message(Box::new(move |msg: DataChannelMessage| {
                        let inbox = Arc::clone(&inbox_for_dc);
                        Box::pin(async move {
                            tracing::trace!(
                                target = "webrtc",
                                event = "handshake_channel_inbound_raw",
                                bytes = msg.data.len(),
                                preview = %hex_preview(&msg.data),
                                "secure handshake received raw datachannel message (answerer)"
                            );
                            inbox.push(msg.data.to_vec()).await;
                        })
                    }));
                    inbox
                };

                let dc_for_handshake = Arc::clone(&dc);
                let channel_state = dc_for_handshake.ready_state();
                let sender_holder = Arc::clone(&secure_sender);
                let local_peer = assigned_peer_value.clone();
                let remote_peer = remote_peer_value.clone();
                let handshake_id_value = (*handshake_value).clone();
                tracing::info!(

                    label = HANDSHAKE_CHANNEL_LABEL,
                    ?channel_state,
                    handshake_id = %handshake_id_value,
                    local_peer = %local_peer,
                    remote_peer = %remote_peer,
                    "spawning handshake task for answerer"
                );
                tracing::trace!(

                    role = "answerer",
                    handshake_id = %handshake_id_value,
                    event = "handshake_task_spawn",
                    thread = %current_thread_label(),
                    "spawning answerer handshake task on global executor"
                );
                spawn_on_global(async move {
                    tracing::info!(

                        handshake_id = %handshake_id_value,
                        local_peer = %local_peer,
                        remote_peer = %remote_peer,
                        "handshake task started on answerer (inside spawned task)"
                    );
                    tracing::debug!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        "answerer handshake task awaiting pre-shared key"
                    );
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        event = "pre_shared_key_wait",
                        thread = %current_thread_label()
                    );
                    let handshake_key = match await_pre_shared_key(
                        &pre_shared_key_cell_value,
                        &session_key_cell_value,
                        Some(passphrase.as_str()),
                        session_id_value.as_str(),
                        handshake_id_value.as_str(),
                    )
                    .await
                    {
                        Ok(Some(key)) => key,
                        Ok(None) => {
                            tracing::error!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                "answerer handshake task could not obtain pre-shared key"
                            );
                            tracing::trace!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                event = "secure_sender_lock_wait",
                                reason = "handshake_key_unavailable",
                                thread = %current_thread_label()
                            );
                            let mut sender_guard = sender_holder.lock().await;
                            tracing::trace!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                event = "secure_sender_lock_acquired",
                                reason = "handshake_key_unavailable",
                                thread = %current_thread_label()
                            );
                            if let Some(sender) = sender_guard.take() {
                                let _ = sender.send(Err(TransportError::Setup(
                                    "handshake key unavailable for secure transport".into(),
                                )));
                            }
                            return;
                        }
                        Err(err) => {
                            tracing::error!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                error = %err,
                                "answerer handshake task failed while deriving pre-shared key"
                            );
                            tracing::trace!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                event = "secure_sender_lock_wait",
                                reason = "handshake_key_error",
                                thread = %current_thread_label()
                            );
                            let mut sender_guard = sender_holder.lock().await;
                            tracing::trace!(

                                role = "answerer",
                                handshake_id = %handshake_id_value,
                                event = "secure_sender_lock_acquired",
                                reason = "handshake_key_error",
                                thread = %current_thread_label()
                            );
                            if let Some(sender) = sender_guard.take() {
                                let _ = sender.send(Err(err));
                            }
                            return;
                        }
                    };
                    tracing::debug!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        key_path = "handshake_for_noise",
                        handshake_hash = %truncated_key_hash(handshake_key.as_ref()),
                        "acquired handshake key for Noise handshake"
                    );
                    let prologue_context = build_prologue_context(
                        &handshake_id_value,
                        local_peer.as_str(),
                        remote_peer.as_str(),
                    );
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        event = "pre_shared_key_acquired",
                        thread = %current_thread_label()
                    );
                    let params = HandshakeParams {
                        handshake_key,
                        handshake_id: handshake_id_value.clone(),
                        local_peer_id: local_peer.clone(),
                        remote_peer_id: remote_peer.clone(),
                        prologue_context,
                        inbox: Arc::clone(&inbox),
                    };
                    tracing::debug!(

                        handshake_id = %handshake_id_value,
                        "calling run_handshake as Responder"
                    );
                    let outcome = match run_handshake(
                        HandshakeRole::Responder,
                        Arc::clone(&dc_for_handshake),
                        params,
                    )
                    .await
                    {
                        Ok(result) => {
                            tracing::info!(

                                handshake_id = %handshake_id_value,
                                "answerer handshake completed successfully as Responder"
                            );
                            Ok::<Option<Arc<HandshakeResult>>, TransportError>(Some(Arc::new(
                                result,
                            )))
                        }
                        Err(err) => {
                            tracing::warn!(

                                handshake_id = %handshake_id_value,
                                error = %err,
                                "answerer handshake failed"
                            );
                            Err(err)
                        }
                    };
                    tracing::info!(

                        handshake_id = %handshake_id_value,
                        outcome = ?outcome.as_ref().map(|_| "success").unwrap_or("error"),
                        "handshake task completed, sending result"
                    );
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        event = "secure_sender_lock_wait",
                        reason = "handshake_result_delivery",
                        thread = %current_thread_label()
                    );
                    let mut sender_guard = sender_holder.lock().await;
                    tracing::trace!(

                        role = "answerer",
                        handshake_id = %handshake_id_value,
                        event = "secure_sender_lock_acquired",
                        reason = "handshake_result_delivery",
                        thread = %current_thread_label()
                    );
                    if let Some(sender) = sender_guard.take() {
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
                target = "beach::transport::webrtc",
                role = "answerer",
                await = "slot.lock",
                state = "start"
            );
            tracing::trace!(

                role = "answerer",
                event = "transport_slot_lock_wait",
                handshake_id = %handshake_value.as_str(),
                thread = %current_thread_label()
            );
            let mut slot_guard = slot.lock().await;
            tracing::trace!(

                role = "answerer",
                event = "transport_slot_lock_acquired",
                handshake_id = %handshake_value.as_str(),
                thread = %current_thread_label(),
                populated = slot_guard.is_some()
            );
            tracing::debug!(
                target = "beach::transport::webrtc",
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
                raw_mode,
            ));
            let transport_dyn: Arc<dyn Transport> = transport.clone();

            if slot_guard.is_none() {
                slot_guard.replace(transport.clone());
                drop(slot_guard);

                if !secure_transport_active {
                    tracing::debug!(role = "answerer", "sending __ready__ sentinel to offerer");

                    if let Err(err) = transport_dyn.send_text("__ready__") {
                        tracing::warn!(

                            error = %err,
                            "answerer readiness ack failed"
                        );
                    } else {
                        tracing::info!(role = "answerer", "sent __ready__ sentinel successfully");
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
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "start"
    );
    pc.set_remote_description(offer_desc)
        .await
        .map_err(to_setup_error)?;
    tracing::info!(
        target = "beach::transport::webrtc",
        role = "answerer",
        handshake_id = %handshake_id.as_str(),
        "offer applied"
    );
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "end"
    );

    // Process queued ICE candidates now that remote description is set
    let pending_candidates = {
        tracing::trace!(

            role = "answerer",
            event = "pending_ice_queue_lock_wait",
            reason = "drain_after_remote_description",
            thread = %current_thread_label()
        );
        let mut queue = pending_for_incoming_clone.lock().await;
        tracing::trace!(

            role = "answerer",
            event = "pending_ice_queue_lock_acquired",
            reason = "drain_after_remote_description",
            thread = %current_thread_label(),
            queue_len = queue.len()
        );
        let candidates = queue.drain(..).collect::<Vec<_>>();
        candidates
    };
    if !pending_candidates.is_empty() {
        tracing::debug!(
            role = "answerer",
            count = pending_candidates.len(),
            "processing queued remote ice candidates"
        );
        for init in pending_candidates {
            if let Err(err) = pc.add_ice_candidate(init).await {
                tracing::warn!(

                    role = "answerer",
                    error = %err,
                    "failed to add queued remote ice candidate"
                );
            }
        }
    }

    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "pc.create_answer",
        state = "start"
    );
    let answer_result = pc.create_answer(None).await;
    tracing::debug!(
        target = "beach::transport::webrtc",
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
        shared_key.as_ref().map(|key| key.as_ref()),
    )?;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "start"
    );
    pc.set_local_description(answer)
        .await
        .map_err(to_setup_error)?;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "end"
    );
    wait_for_local_description(&pc).await?;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "start"
    );
    let post_result = post_sdp(&client, signaling_url, "answer", &[], &answer_payload).await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "end",
        result = ?post_result
    );
    post_result?;
    signaling_client
        .unlock_remote_peer(&expected_remote_peer)
        .await;

    let mut attempts: usize = 0;
    let max_attempts = 1000; // 10 seconds timeout (1000 * 10ms)
    let transport = loop {
        attempts = attempts.saturating_add(1);
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach::transport::webrtc",
                role = "answerer",
                await = "transport_slot.lock",
                state = "start",
                attempts
            );
        }
        tracing::trace!(

            role = "answerer",
            event = "transport_slot_lock_wait",
            attempts,
            thread = %current_thread_label()
        );
        let mut transport_guard = transport_slot.lock().await;
        tracing::trace!(

            role = "answerer",
            event = "transport_slot_lock_acquired",
            attempts,
            thread = %current_thread_label(),
            has_transport = transport_guard.is_some()
        );
        if attempts == 1 || attempts % 100 == 0 {
            tracing::debug!(
                target = "beach::transport::webrtc",
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
                target = "beach::transport::webrtc",
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
                target = "beach::transport::webrtc",
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
                target = "beach::transport::webrtc",
                role = "answerer",
                await = "sleep.retry",
                state = "end",
                attempts
            );
        }
    };
    tracing::debug!(target = "webrtc", ?client_id, "answerer transport ready");

    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "start"
    );
    let wait_result = wait_for_connection(&pc).await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "end",
        result = ?wait_result
    );
    wait_result?;

    // Check if transport is already populated (data channel already opened)
    let already_ready = {
        tracing::trace!(

            role = "answerer",
            event = "transport_slot_lock_wait",
            reason = "already_ready_check",
            thread = %current_thread_label()
        );
        let guard = transport_slot.lock().await;
        let populated = guard.is_some();
        tracing::trace!(

            role = "answerer",
            event = "transport_slot_lock_acquired",
            reason = "already_ready_check",
            thread = %current_thread_label(),
            has_transport = populated
        );
        drop(guard);
        populated
    };

    if !already_ready {
        tracing::debug!(
            target = "beach::transport::webrtc",
            role = "answerer",
            await = "dc_open_notify.timeout",
            state = "start"
        );
        let notify_result = timeout(CONNECT_TIMEOUT, dc_open_notify.notified()).await;
        tracing::debug!(
            target = "beach::transport::webrtc",
            role = "answerer",
            await = "dc_open_notify.timeout",
            state = "end",
            result = ?notify_result
        );
        notify_result.map_err(|_| TransportError::Timeout)?;
    } else {
        tracing::debug!(
            target = "beach::transport::webrtc",
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
            role = "answerer",
            "sending encrypted __ready__ sentinel to offerer"
        );
        if let Err(err) = transport.send_text("__ready__") {
            tracing::warn!(

                error = %err,
                "answerer encrypted readiness ack failed"
            );
        }
    }
    let transport_dyn: Arc<dyn Transport> = transport.clone();
    let mut connection_metadata = signaling_client.remote_metadata().await.unwrap_or_default();
    validate_manager_peer(&connection_metadata, &session_id).await?;
    if let Some(local_meta) = build_label_metadata(label) {
        for (key, value) in local_meta {
            connection_metadata.entry(key).or_insert(value);
        }
    }
    let metadata = if connection_metadata.is_empty() {
        None
    } else {
        Some(connection_metadata)
    };
    tracing::info!(
        target = "beach::transport::webrtc",
        role = "answerer",
        session_id = %session_id,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "webrtc answerer connected"
    );

    Ok(WebRtcConnection::new(
        transport_dyn,
        channels,
        secure_context,
        Some(Arc::clone(&signaling_client)),
        metadata,
    ))
}

fn build_label_metadata(label: Option<&str>) -> Option<HashMap<String, String>> {
    let trimmed = label?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut map = HashMap::new();
    map.insert("label".to_string(), trimmed.to_string());
    map.insert("role".to_string(), "manager".to_string());
    Some(map)
}

// Manager JWT validation ----------------------------------------------------

#[derive(Clone)]
struct ManagerAuthAuthority {
    jwks_url: String,
    issuer: String,
    audience: String,
}

struct ManagerJwtVerifier {
    authority: ManagerAuthAuthority,
    cache: AsyncMutex<Option<JwksCache>>,
    client: Client,
}

#[derive(Clone)]
struct JwksCache {
    keys: HashMap<String, CachedDecodingKey>,
    fetched_at: Instant,
}

#[derive(Clone)]
struct CachedDecodingKey {
    key: DecodingKey,
    algorithm: Algorithm,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
    #[serde(default)]
    crv: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct ManagerClaims {
    #[serde(default)]
    iss: Option<String>,
    #[serde(default)]
    aud: Option<serde_json::Value>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(default)]
    exp: Option<i64>,
}

static MANAGER_VERIFIER: SyncOnceCell<Option<ManagerJwtVerifier>> = SyncOnceCell::new();

async fn validate_manager_peer(
    metadata: &HashMap<String, String>,
    expected_session_id: &str,
) -> Result<(), TransportError> {
    let role = metadata.get("role").map(|s| s.as_str()).unwrap_or_default();
    if role != "manager" {
        return Ok(());
    }
    let bearer = metadata
        .get("bearer")
        .ok_or_else(|| TransportError::Setup("missing bearer in manager metadata".into()))?;
    let session_id = metadata
        .get("session_id")
        .ok_or_else(|| TransportError::Setup("missing session_id in manager metadata".into()))?;
    if session_id != expected_session_id {
        return Err(TransportError::Setup(format!(
            "manager metadata session mismatch: expected {}, got {}",
            expected_session_id, session_id
        )));
    }
    let verifier = MANAGER_VERIFIER
        .get_or_init(ManagerJwtVerifier::from_env)
        .as_ref()
        .ok_or_else(|| {
            TransportError::Setup(
                "manager JWT verifier unavailable (missing Clerk/Beach Gate config)".into(),
            )
        })?;
    verifier.verify(bearer).await
}

impl ManagerJwtVerifier {
    fn from_env() -> Option<Self> {
        let jwks_url = std::env::var("CLERK_JWKS_URL")
            .or_else(|_| std::env::var("BEACH_GATE_JWKS_URL"))
            .ok()?;
        let issuer = std::env::var("CLERK_ISSUER")
            .or_else(|_| std::env::var("BEACH_GATE_ISSUER"))
            .ok()?;
        let audience = std::env::var("CLERK_AUDIENCE")
            .or_else(|_| std::env::var("BEACH_GATE_AUDIENCE"))
            .ok()?;
        Some(Self {
            authority: ManagerAuthAuthority {
                jwks_url,
                issuer,
                audience,
            },
            cache: AsyncMutex::new(None),
            client: Client::new(),
        })
    }

    async fn verify(&self, token: &str) -> Result<(), TransportError> {
        let header = decode_header(token)
            .map_err(|err| TransportError::Setup(format!("invalid manager jwt header: {err}")))?;
        let kid = header
            .kid
            .ok_or_else(|| TransportError::Setup("manager jwt missing kid".into()))?;
        let key = self.decoding_key(&kid).await?;
        let mut validation = Validation::new(key.algorithm);
        validation.set_issuer(&[self.authority.issuer.clone()]);
        validation.set_audience(&[self.authority.audience.clone()]);
        decode::<ManagerClaims>(token, &key.key, &validation).map_err(|err| {
            TransportError::Setup(format!("manager jwt validation failed: {err}"))
        })?;
        Ok(())
    }

    async fn decoding_key(&self, kid: &str) -> Result<CachedDecodingKey, TransportError> {
        {
            let cache = self.cache.lock().await;
            if let Some(store) = cache.as_ref() {
                if !store.fetched_at.elapsed().is_zero() {
                    if let Some(key) = store.keys.get(kid) {
                        return Ok(key.clone());
                    }
                }
            }
        }

        let mut cache = self.cache.lock().await;
        let fetched = self.fetch_jwks().await?;
        *cache = Some(fetched);
        let cache = cache.as_ref().expect("cache set");
        cache
            .keys
            .get(kid)
            .cloned()
            .ok_or_else(|| TransportError::Setup(format!("jwks missing requested kid {kid}")))
    }

    async fn fetch_jwks(&self) -> Result<JwksCache, TransportError> {
        let resp = self
            .client
            .get(&self.authority.jwks_url)
            .send()
            .await
            .map_err(|err| TransportError::Setup(format!("jwks fetch failed: {err}")))?;
        let resp = resp.error_for_status().map_err(|err| {
            TransportError::Setup(format!(
                "jwks http error: {}",
                err.status().unwrap_or_default()
            ))
        })?;
        let body: JwksResponse = resp
            .json()
            .await
            .map_err(|err| TransportError::Setup(format!("jwks parse failed: {err}")))?;
        let mut keys = HashMap::new();
        for key in body.keys {
            match key.kty.as_str() {
                "RSA" => {
                    let (Some(n), Some(e)) = (key.n, key.e) else {
                        continue;
                    };
                    let decoding_key = DecodingKey::from_rsa_components(&n, &e).map_err(|err| {
                        TransportError::Setup(format!("jwks rsa key error: {err}"))
                    })?;
                    keys.insert(
                        key.kid,
                        CachedDecodingKey {
                            key: decoding_key,
                            algorithm: Algorithm::RS256,
                        },
                    );
                }
                "EC" => {
                    if key.crv.as_deref() != Some("P-256") {
                        continue;
                    }
                    let (Some(x), Some(y)) = (key.x, key.y) else {
                        continue;
                    };
                    let decoding_key = DecodingKey::from_ec_components(&x, &y).map_err(|err| {
                        TransportError::Setup(format!("jwks ec key error: {err}"))
                    })?;
                    keys.insert(
                        key.kid,
                        CachedDecodingKey {
                            key: decoding_key,
                            algorithm: Algorithm::ES256,
                        },
                    );
                }
                _ => continue,
            }
        }
        if keys.is_empty() {
            return Err(TransportError::Setup(
                "jwks fetch returned no usable keys".into(),
            ));
        }
        Ok(JwksCache {
            keys,
            fetched_at: Instant::now(),
        })
    }
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
    let url_string = url.as_str().to_string();
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "start",
        url = %url_string
    );
    let send_attempt = client.post(url.clone()).json(payload).send().await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status),
        url = %url_string
    );
    let response = send_attempt.map_err(http_error)?;

    if response.status().is_success() {
        return Ok(());
    }
    Err(TransportError::Setup(format!(
        "unexpected signaling status {}",
        response.status()
    )))
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
    let url_string = url.as_str().to_string();
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "start",
        url = %url_string
    );
    let send_attempt = client.get(url.clone()).send().await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status),
        url = %url_string
    );
    let response = send_attempt.map_err(http_error)?;

    match response.status() {
        StatusCode::OK => {
            tracing::debug!(
                target = "beach::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "start"
            );
            let payload_attempt = response.json::<WebRtcSdpPayload>().await;
            tracing::debug!(
                target = "beach::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "end",
                success = payload_attempt.is_ok()
            );
            let payload = payload_attempt.map_err(http_error)?;
            Ok(Some(payload))
        }
        StatusCode::NO_CONTENT | StatusCode::NOT_FOUND => Ok(None),
        status if status.is_server_error() => Err(TransportError::Setup(format!(
            "signaling server returned {status}"
        ))),
        status => Err(TransportError::Setup(format!(
            "unexpected signaling status {status}"
        ))),
    }
}

fn extract_session_id(signaling_url: &str) -> Result<String, TransportError> {
    let url = Url::parse(signaling_url).map_err(|err| {
        TransportError::Setup(format!("invalid signaling url {signaling_url}: {err}"))
    })?;
    let segments = url
        .path_segments()
        .ok_or_else(|| TransportError::Setup("signaling url missing path segments".into()))?
        .collect::<Vec<_>>();
    if segments.len() < 2 || segments[0] != "sessions" {
        return Err(TransportError::Setup(format!(
            "unexpected signaling url path: {}",
            url.path()
        )));
    }
    Ok(segments[1].to_string())
}

fn truncated_key_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(&digest[..8])
}

fn current_thread_label() -> String {
    format!("{:?}", std::thread::current().id())
}

#[cfg(test)]
static SESSION_KEY_DERIVE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct SessionKeyInflight {
    result: AsyncMutex<Option<Result<Arc<[u8; 32]>, Arc<TransportError>>>>,
    notify: Notify,
    started: AtomicBool,
}

static SESSION_KEY_SINGLEFLIGHT: Lazy<Mutex<HashMap<usize, Arc<SessionKeyInflight>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

async fn derive_session_key_singleflight(
    cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    passphrase: &str,
    session_id: &str,
) -> Result<Arc<[u8; 32]>, TransportError> {
    if let Some(existing) = cell.get() {
        tracing::debug!(
            session_id = %session_id,
            key_path = "session_cache",
            session_hash = %truncated_key_hash(existing.as_ref()),
            "using cached session key"
        );
        return Ok(existing.clone());
    }

    let key = Arc::as_ptr(cell) as usize;
    let inflight = {
        let mut guard = SESSION_KEY_SINGLEFLIGHT.lock().unwrap();
        guard
            .entry(key)
            .or_insert_with(|| Arc::new(SessionKeyInflight::default()))
            .clone()
    };

    loop {
        if let Some(existing) = cell.get() {
            return Ok(existing.clone());
        }

        let guard = inflight.result.lock().await;
        if let Some(result) = guard.as_ref() {
            let cloned = result
                .clone()
                .map_err(|err| TransportError::Setup(err.to_string()))?;
            return Ok(cloned);
        }

        // We are the first waiter; perform derivation.
        if inflight.started.swap(true, Ordering::SeqCst) {
            drop(guard);
            inflight.notify.notified().await;
            continue;
        }
        #[cfg(test)]
        test_note_session_key_derivation();
        let passphrase_owned = passphrase.to_string();
        let session_id_owned = session_id.to_string();
        drop(guard);
        let derived = tokio::task::spawn_blocking(move || {
            derive_pre_shared_key(passphrase_owned.as_str(), session_id_owned.as_str())
        })
        .await
        .map_err(|err| {
            TransportError::Setup(format!("session key derivation task failed: {err}"))
        })?;

        let result = derived.map(Arc::new).map_err(Arc::new);

        let mut guard = inflight.result.lock().await;
        guard.replace(result.clone());
        inflight.notify.notify_waiters();

        if let Ok(derived_key) = result
            .clone()
            .map_err(|err| TransportError::Setup(err.to_string()))
        {
            let _ = cell.set(derived_key.clone());
            let mut map_guard = SESSION_KEY_SINGLEFLIGHT.lock().unwrap();
            map_guard.remove(&key);
            drop(map_guard);
            return Ok(cell.get().cloned().unwrap_or(derived_key));
        } else {
            let mut map_guard = SESSION_KEY_SINGLEFLIGHT.lock().unwrap();
            map_guard.remove(&key);
            drop(map_guard);
            return result.map_err(|err| TransportError::Setup(err.to_string()));
        }
    }
}

async fn ensure_session_key(
    cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    passphrase: Option<&str>,
    session_id: &str,
) -> Result<Option<Arc<[u8; 32]>>, TransportError> {
    let Some(passphrase_value) = passphrase else {
        return Ok(None);
    };
    if !should_encrypt(Some(passphrase_value)) {
        return Ok(None);
    }
    let result = derive_session_key_singleflight(cell, passphrase_value, session_id).await?;
    Ok(Some(result))
}

fn prime_session_key(
    cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    passphrase: Option<&str>,
    session_id: &str,
) {
    let Some(passphrase_value) = passphrase else {
        return;
    };
    if !should_encrypt(Some(passphrase_value)) || cell.get().is_some() {
        return;
    }
    let cell_clone = Arc::clone(cell);
    let passphrase_owned = passphrase_value.to_string();
    let session_id_owned = session_id.to_string();
    spawn_on_global(async move {
        if cell_clone.get().is_some() {
            return;
        }
        if let Err(err) = ensure_session_key(
            &cell_clone,
            Some(passphrase_owned.as_str()),
            session_id_owned.as_str(),
        )
        .await
        {
            tracing::error!(

                session_id = %session_id_owned,
                error = %err,
                "background session key derivation failed"
            );
        }
    });
}

#[cfg(test)]
fn test_reset_session_key_derivations() {
    SESSION_KEY_DERIVE_INVOCATIONS.store(0, Ordering::SeqCst);
}

#[cfg(test)]
fn test_session_key_derivations() -> usize {
    SESSION_KEY_DERIVE_INVOCATIONS.load(Ordering::SeqCst)
}

#[cfg(test)]
fn test_note_session_key_derivation() {
    SESSION_KEY_DERIVE_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
}

fn prime_pre_shared_key(
    handshake_cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    session_key_cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    passphrase: Option<&str>,
    session_id: &str,
    handshake_id: &str,
) {
    let Some(passphrase_value) = passphrase else {
        return;
    };
    if !should_encrypt(Some(passphrase_value)) || handshake_cell.get().is_some() {
        return;
    }
    prime_session_key(session_key_cell, Some(passphrase_value), session_id);
    let handshake_cell_clone = Arc::clone(handshake_cell);
    let session_key_cell_clone = Arc::clone(session_key_cell);
    let passphrase_owned = passphrase_value.to_string();
    let session_id_owned = session_id.to_string();
    let handshake_owned = handshake_id.to_string();
    spawn_on_global(async move {
        if handshake_cell_clone.get().is_some() {
            return;
        }
        match ensure_session_key(
            &session_key_cell_clone,
            Some(passphrase_owned.as_str()),
            session_id_owned.as_str(),
        )
        .await
        {
            Ok(Some(session_key)) => match derive_handshake_key_from_session(
                session_key.as_ref(),
                handshake_owned.as_str(),
            ) {
                Ok(derived) => {
                    let arc_key = Arc::new(derived);
                    let session_hash = truncated_key_hash(session_key.as_ref());
                    let handshake_hash = truncated_key_hash(arc_key.as_ref());
                    if handshake_cell_clone.set(arc_key.clone()).is_ok() {
                        tracing::debug!(

                            handshake_id = %handshake_owned,
                            key_path = "background_precompute",
                            session_hash = %session_hash,
                            handshake_hash = %handshake_hash,
                            "background handshake key derivation complete"
                        );
                    }
                }
                Err(err) => {
                    tracing::error!(

                        handshake_id = %handshake_owned,
                        error = %err,
                        "handshake key derivation failed"
                    );
                }
            },
            Ok(None) => {}
            Err(err) => {
                tracing::error!(

                    handshake_id = %handshake_owned,
                    error = %err,
                    "background session key derivation for handshake failed"
                );
            }
        }
    });
}

async fn await_pre_shared_key(
    handshake_cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    session_key_cell: &Arc<OnceCell<Arc<[u8; 32]>>>,
    passphrase: Option<&str>,
    session_id: &str,
    handshake_id: &str,
) -> Result<Option<Arc<[u8; 32]>>, TransportError> {
    if let Some(existing) = handshake_cell.get() {
        tracing::debug!(

            handshake_id = %handshake_id,
            key_path = "handshake_cache",
            key_hash = %truncated_key_hash(existing.as_ref()),
            "using cached handshake key"
        );
        return Ok(Some(existing.clone()));
    }
    let Some(passphrase_value) = passphrase else {
        tracing::debug!(

            handshake_id = %handshake_id,
            key_path = "passphrase_missing",
            "handshake pre-shared key unavailable; no passphrase provided"
        );
        return Ok(None);
    };
    if !should_encrypt(Some(passphrase_value)) {
        tracing::debug!(

            handshake_id = %handshake_id,
            key_path = "encryption_disabled",
            "handshake pre-shared key bypassed; encryption disabled"
        );
        return Ok(None);
    }
    let session_cached_before = session_key_cell.get().is_some();
    let session_key =
        ensure_session_key(session_key_cell, Some(passphrase_value), session_id).await?;
    let Some(session_key) = session_key else {
        tracing::warn!(

            handshake_id = %handshake_id,
            key_path = "session_key_missing",
            "expected session key but none was returned"
        );
        return Ok(None);
    };
    let session_key_hash = truncated_key_hash(session_key.as_ref());
    let session_source: &'static str = if session_cached_before {
        "cache"
    } else if session_key_cell
        .get()
        .map(|existing| Arc::ptr_eq(existing, &session_key))
        .unwrap_or(false)
    {
        "derived"
    } else {
        "race"
    };
    let derived = derive_handshake_key_from_session(session_key.as_ref(), handshake_id)?;
    let handshake_hash = truncated_key_hash(&derived);
    let arc_key = Arc::new(derived);
    match handshake_cell.set(arc_key.clone()) {
        Ok(()) => {
            tracing::debug!(

                handshake_id = %handshake_id,
                key_path = "derived_from_session",
                session_source = %session_source,
                session_hash = %session_key_hash,
                handshake_hash = %handshake_hash,
                "handshake key cached"
            );
            Ok(Some(arc_key))
        }
        Err(SetError::AlreadyInitializedError(_)) => {
            let cached_hash = handshake_cell
                .get()
                .map(|existing| truncated_key_hash(existing.as_ref()));
            let cached_hash = cached_hash.as_deref().unwrap_or("unknown");
            tracing::debug!(

                handshake_id = %handshake_id,
                key_path = "handshake_cache_race",
                session_source = %session_source,
                session_hash = %session_key_hash,
                attempted_hash = %handshake_hash,
                cached_hash,
                "handshake key already initialized"
            );
            Ok(handshake_cell.get().cloned().or(Some(arc_key)))
        }
        Err(SetError::InitializingError(value)) => {
            let pending_hash = truncated_key_hash(value.as_ref());
            tracing::debug!(

                handshake_id = %handshake_id,
                key_path = "handshake_initializing",
                session_source = %session_source,
                session_hash = %session_key_hash,
                pending_hash = %pending_hash,
                "handshake key initialization in progress"
            );
            Ok(Some(value))
        }
    }
}

fn payload_from_description(
    desc: &RTCSessionDescription,
    handshake_id: &str,
    from_peer: &str,
    to_peer: &str,
    passphrase: Option<&str>,
    pre_shared_key: Option<&[u8; 32]>,
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
            tracing::debug!(

                handshake_id = %handshake_id,
                from_peer,
                to_peer,
                typ = typ.as_str(),
                "offer sealing associated data"
            );
            let sealed = if let Some(psk) = pre_shared_key {
                seal_message_with_psk(psk, handshake_id, label, &associated, desc.sdp.as_bytes())?
            } else {
                seal_message(
                    passphrase_value,
                    handshake_id,
                    label,
                    &associated,
                    desc.sdp.as_bytes(),
                )?
            };
            tracing::debug!(

                handshake_id = %handshake_id,
                nonce = sealed.nonce,
                ciphertext_len = sealed.ciphertext.len(),
                plaintext_len = desc.sdp.len(),
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
    pre_shared_key: Option<&[u8; 32]>,
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
        let plaintext = if let Some(psk) = pre_shared_key {
            open_message_with_psk(psk, &payload.handshake_id, label, &associated, sealed)?
        } else {
            open_message(
                passphrase_value,
                &payload.handshake_id,
                label,
                &associated,
                sealed,
            )?
        };
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
    pre_shared_key: Option<&[u8; 32]>,
    handshake_id: &str,
    from_peer: &str,
    to_peer: &str,
) -> Result<IceCandidateBlob, TransportError> {
    if let Some(psk) = pre_shared_key {
        tracing::debug!(

            handshake_id = %handshake_id,
            key_path = "handshake_pre_shared",
            key_hash = %truncated_key_hash(psk),
            from_peer,
            to_peer,
            "pre-shared key provided for ice candidate"
        );
    } else if sealed.is_some() {
        tracing::debug!(

            handshake_id = %handshake_id,
            key_path = "passphrase_fallback",
            from_peer,
            to_peer,
            "sealed ice candidate will use passphrase fallback"
        );
    } else {
        tracing::trace!(

            handshake_id = %handshake_id,
            from_peer,
            to_peer,
            "plain ice candidate; no key required"
        );
    }
    if let Some(sealed_env) = sealed {
        let passphrase_value = passphrase.ok_or_else(|| {
            TransportError::Setup("missing passphrase for sealed ice candidate".into())
        })?;
        let associated = [from_peer, to_peer, handshake_id];
        tracing::debug!(
            target = "webrtc",
            handshake_id = %handshake_id,
            from_peer,
            to_peer,
            aad_from = %from_peer,
            aad_to = %to_peer,
            aad_label = "ice",
            ciphertext_len = sealed_env.ciphertext.len(),
            nonce = sealed_env.nonce.as_str(),
            using_pre_shared_key = pre_shared_key.is_some(),
            "attempting to open sealed ice candidate"
        );
        let plaintext = if let Some(psk) = pre_shared_key {
            match open_message_with_psk(
                psk,
                handshake_id,
                MessageLabel::Ice,
                &associated,
                &sealed_env,
            ) {
                Ok(bytes) => {
                    tracing::debug!(
                        target = "webrtc",
                        handshake_id,
                        from_peer,
                        to_peer,
                        "sealed ice candidate decrypted with handshake key"
                    );
                    bytes
                }
                Err(err) => {
                    let handshake_error = err.to_string();
                    tracing::warn!(
                        handshake_id,
                        from_peer,
                        to_peer,
                        error = %handshake_error,
                        "sealed ice candidate decrypt failed with handshake key; falling back to passphrase"
                    );
                    match open_message(
                        passphrase_value,
                        handshake_id,
                        MessageLabel::Ice,
                        &associated,
                        &sealed_env,
                    ) {
                        Ok(bytes) => {
                            tracing::debug!(
                                target = "webrtc",
                                handshake_id,
                                from_peer,
                                to_peer,
                                "sealed ice candidate decrypted with passphrase fallback"
                            );
                            bytes
                        }
                        Err(passphrase_err) => {
                            let passphrase_error = passphrase_err.to_string();
                            tracing::error!(
                                handshake_id,
                                from_peer,
                                to_peer,
                                handshake_error = %handshake_error,
                                passphrase_error = %passphrase_error,
                                "sealed ice candidate decrypt failed with handshake key and passphrase"
                            );
                            return Err(TransportError::Setup(format!(
                                "sealed ice candidate decrypt failed: handshake_key_error={handshake_error}; passphrase_error={passphrase_error}"
                            )));
                        }
                    }
                }
            }
        } else {
            let bytes = open_message(
                passphrase_value,
                handshake_id,
                MessageLabel::Ice,
                &associated,
                &sealed_env,
            )?;
            tracing::debug!(
                target = "webrtc",
                handshake_id,
                from_peer,
                to_peer,
                "sealed ice candidate decrypted with passphrase (no handshake key available)"
            );
            bytes
        };
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
        .create_data_channel("beach", Some(dc_init))
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
        false,
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
        false,
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

#[derive(Debug)]
struct PeerConnectionTraceContext {
    role: &'static str,
    handshake_id: Option<String>,
    session_id: Option<String>,
    remote_peer: Option<String>,
    transport_id: Option<TransportId>,
}

fn install_peer_connection_tracing(
    pc: &Arc<RTCPeerConnection>,
    role: &'static str,
    handshake_id: Option<String>,
    session_id: Option<String>,
    remote_peer: Option<String>,
    transport_id: Option<TransportId>,
) {
    let context = Arc::new(PeerConnectionTraceContext {
        role,
        handshake_id,
        session_id,
        remote_peer,
        transport_id,
    });
    let role_label = role.to_string();

    let peer_ctx = context.clone();
    pc.on_peer_connection_state_change(Box::new(move |state| {
        let ctx = peer_ctx.clone();
        let role = role_label.clone();
        Box::pin(async move {
            tracing::trace!(
                target = "beach::transport::webrtc",
                role = ctx.role,
                handshake_id = ctx.handshake_id.as_deref().unwrap_or(""),
                session_id = ctx.session_id.as_deref().unwrap_or(""),
                remote_peer = ctx.remote_peer.as_deref().unwrap_or(""),
                transport_id = ?ctx.transport_id,
                new_state = ?state,
                "peer connection state change"
            );
            if state == RTCPeerConnectionState::Failed {
                metrics::WEBRTC_DTLS_FAILURES
                    .with_label_values(&[&role])
                    .inc();
            }
        })
    }));

    let ice_ctx = context.clone();
    pc.on_ice_connection_state_change(Box::new(move |state| {
        let ctx = ice_ctx.clone();
        Box::pin(async move {
            tracing::trace!(
                target = "beach::transport::webrtc",
                role = ctx.role,
                handshake_id = ctx.handshake_id.as_deref().unwrap_or(""),
                session_id = ctx.session_id.as_deref().unwrap_or(""),
                remote_peer = ctx.remote_peer.as_deref().unwrap_or(""),
                transport_id = ?ctx.transport_id,
                new_state = ?state,
                "peer connection ice connection state change"
            );
        })
    }));

    let signal_ctx = context.clone();
    pc.on_signaling_state_change(Box::new(move |state| {
        let ctx = signal_ctx.clone();
        Box::pin(async move {
            tracing::trace!(
                target = "beach::transport::webrtc",
                role = ctx.role,
                handshake_id = ctx.handshake_id.as_deref().unwrap_or(""),
                session_id = ctx.session_id.as_deref().unwrap_or(""),
                remote_peer = ctx.remote_peer.as_deref().unwrap_or(""),
                transport_id = ?ctx.transport_id,
                new_state = ?state,
                "peer connection signaling state change"
            );
        })
    }));

    let gather_ctx = context;
    pc.on_ice_gathering_state_change(Box::new(move |state| {
        let ctx = gather_ctx.clone();
        Box::pin(async move {
            tracing::trace!(
                target = "beach::transport::webrtc",
                role = ctx.role,
                handshake_id = ctx.handshake_id.as_deref().unwrap_or(""),
                session_id = ctx.session_id.as_deref().unwrap_or(""),
                remote_peer = ctx.remote_peer.as_deref().unwrap_or(""),
                transport_id = ?ctx.transport_id,
                new_state = ?state,
                "peer connection ice gathering state change"
            );
        })
    }));
}

async fn wait_for_local_description(pc: &Arc<RTCPeerConnection>) -> Result<(), TransportError> {
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "start"
    );
    let already_present = pc.local_description().await.is_some();
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "end",
        has_description = already_present
    );
    if already_present {
        return Ok(());
    }
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "start"
    );
    let mut gather = pc.gathering_complete_promise().await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "end"
    );
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "start"
    );
    let _ = gather.recv().await;
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "end"
    );
    tracing::debug!(
        target = "beach::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.final",
        state = "start"
    );
    let final_present = pc.local_description().await.is_some();
    tracing::debug!(
        target = "beach::transport::webrtc",
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
        target = "beach::transport::webrtc",
        phase = "wait_for_connection",
        await = "timeout(rx)",
        state = "start"
    );
    let wait_result = timeout(CONNECT_TIMEOUT, rx).await;
    tracing::debug!(
        target = "beach::transport::webrtc",
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
    use crate::transport::webrtc::signaling::{PeerRole, TransportType};
    use futures::future::join_all;
    use std::time::Instant;

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

    #[test_timeout::timeout]
    fn webrtc_handshake_completes_within_budget() {
        let started = Instant::now();
        let pair = match build_pair() {
            Ok(pair) => pair,
            Err(err) => {
                tracing::debug!(target = "webrtc", error = %err, "skipping webrtc_handshake_completes_within_budget");
                return;
            }
        };
        let handshake_elapsed = started.elapsed();
        assert!(
            handshake_elapsed < Duration::from_secs(10),
            "local webrtc handshake took too long: {:?}",
            handshake_elapsed
        );
        eprintln!("local webrtc handshake duration: {:?}", handshake_elapsed);
        drop(pair);
    }

    #[test_timeout::timeout]
    fn session_key_derivation_concurrency_is_fast_and_deduplicated() {
        test_reset_session_key_derivations();
        let cell = Arc::new(OnceCell::<Arc<[u8; 32]>>::new());
        let passphrase = "session-key-derivation-test-passphrase";
        let session_id = "session-key-derivation-test-session";

        let started = Instant::now();
        let results = RUNTIME.block_on(async {
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let cell_clone = Arc::clone(&cell);
                tasks.push(tokio::spawn(async move {
                    ensure_session_key(&cell_clone, Some(passphrase), session_id).await
                }));
            }
            join_all(tasks).await
        });
        let elapsed = started.elapsed();

        let mut hashes = Vec::new();
        for result in results {
            let key_opt = result
                .expect("session derivation task panicked")
                .expect("derivation failed");
            let key = key_opt.expect("session key missing despite passphrase");
            hashes.push(truncated_key_hash(key.as_ref()));
        }

        let first = hashes.first().expect("hashes should not be empty").clone();
        assert!(
            hashes.iter().all(|hash| hash == &first),
            "session key derivations returned mismatched keys: {:?}",
            hashes
        );

        let derivations = test_session_key_derivations();
        eprintln!(
            "session key derivation elapsed: {:?}, spawn_blocking invocations: {}",
            elapsed, derivations
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "session key derivation too slow: {:?}",
            elapsed
        );
    }

    #[test]
    fn encryption_manager_reports_enabled_when_state_installed() {
        let manager = EncryptionManager::new();
        let mut send_key = [0u8; 32];
        let mut recv_key = [0u8; 32];
        send_key[0] = 1;
        recv_key[0] = 2;
        let result = HandshakeResult {
            send_key,
            recv_key,
            verification_code: "123456".to_string(),
        };

        manager.enable(&result).expect("enable encryption");
        manager.enabled.store(false, Ordering::SeqCst);

        assert!(
            manager.is_enabled(),
            "encryption manager should report enabled once cipher state is installed"
        );
    }

    #[test]
    fn encrypted_frame_buffered_until_encryption_enabled() {
        use crossbeam_channel::unbounded;

        let encryption = Arc::new(EncryptionManager::new());
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let frame_config = framed::runtime_config().clone();
        let frame_reassembler =
            Arc::new(Mutex::new(framed::FramedDecoder::new(frame_config.clone())));
        let (sender, receiver) = unbounded();
        let log_id = TransportId(7);

        let plaintext_message = TransportMessage::text(1, "__ready__");
        let encoded_payload = encode_message(&plaintext_message);
        let plaintext_frames =
            framed::encode_message("sync", "text", 1, &encoded_payload, &frame_config)
                .expect("frame encode");
        let plaintext_frame = plaintext_frames
            .first()
            .expect("single frame expected")
            .to_vec();

        WebRtcTransport::handle_incoming_bytes(
            &encryption,
            &pending,
            &frame_reassembler,
            plaintext_frame.clone(),
            &sender,
            log_id,
            false,
        );
        let received = receiver.try_recv().expect("plaintext should pass through");
        assert_eq!(received.payload.as_text(), Some("__ready__"));

        let mut offerer_send_key = [0u8; 32];
        offerer_send_key[0] = 1;
        let mut offerer_recv_key = [0u8; 32];
        offerer_recv_key[0] = 2;
        let offerer_result = HandshakeResult {
            send_key: offerer_send_key,
            recv_key: offerer_recv_key,
            verification_code: "offerer".into(),
        };
        let answerer_result = HandshakeResult {
            send_key: offerer_recv_key,
            recv_key: offerer_send_key,
            verification_code: "answerer".into(),
        };
        let remote_encryption = EncryptionManager::new();
        remote_encryption
            .enable(&answerer_result)
            .expect("enable remote encryption");
        let queued_message = TransportMessage::text(2, "__ready__");
        let queued_payload = encode_message(&queued_message);
        let queued_frames =
            framed::encode_message("sync", "text", 2, &queued_payload, &frame_config)
                .expect("frame encode");
        let encrypted_frame = remote_encryption
            .encrypt(
                queued_frames
                    .first()
                    .expect("single frame expected")
                    .as_ref(),
            )
            .expect("encrypt frame");

        WebRtcTransport::handle_incoming_bytes(
            &encryption,
            &pending,
            &frame_reassembler,
            encrypted_frame,
            &sender,
            log_id,
            false,
        );
        assert!(
            matches!(receiver.try_recv(), Err(CrossbeamTryRecvError::Empty)),
            "encrypted frame should be queued until encryption is enabled"
        );
        assert_eq!(pending.lock().unwrap().len(), 1);

        encryption
            .enable(&offerer_result)
            .expect("enable local encryption");
        WebRtcTransport::flush_pending_encrypted_internal(
            &encryption,
            &frame_reassembler,
            &pending,
            &sender,
            log_id,
            false,
        );

        let received = receiver.try_recv().expect("flushed frame delivered");
        assert_eq!(received.payload.as_text(), Some("__ready__"));
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn peer_supports_secure_transport_private_beach_dashboard_label() {
        let mut metadata = HashMap::new();
        metadata.insert("label".to_string(), "private-beach-dashboard".to_string());
        let peer = PeerInfo {
            id: "viewer-peer".to_string(),
            role: PeerRole::Client,
            joined_at: 0,
            supported_transports: vec![TransportType::WebRTC],
            preferred_transport: None,
            metadata: Some(metadata),
        };

        assert!(
            OffererInner::peer_supports_secure_transport(&peer),
            "viewer peers should negotiate secure transport when labeled as private-beach-dashboard"
        );
    }
}

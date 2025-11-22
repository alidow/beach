use std::{
    collections::{hash_map::Entry, HashMap},
    net::IpAddr,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration as StdDuration, Instant},
};

use anyhow::Context;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use beach_buggy::{fast_path::FastPathPayloadKind, ActionAck, ActionCommand, StateDiff};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, RwLock},
    time::{sleep, timeout, Duration},
};
use tracing::{debug, info, trace, warn};

use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::Error as WebRtcError;
use webrtc_ice::{
    network_type::NetworkType,
    udp_network::{EphemeralUDP, UDPNetwork},
};

use crate::{log_throttle::should_log_custom_event, metrics, state::AppState};

static FAST_PATH_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct IceCandidateMeta {
    ip: IpAddr,
    port: u16,
    scope: &'static str,
}

const FAST_PATH_CHUNK_VERSION: u8 = 1;
const FAST_PATH_CHUNK_TTL: StdDuration = StdDuration::from_secs(15);

#[derive(Debug)]
struct PendingChunk {
    parts: Vec<Option<Vec<u8>>>,
    expected: u32,
    received: u32,
    expires_at: Instant,
}

impl PendingChunk {
    fn new(count: u32) -> anyhow::Result<Self> {
        if count == 0 {
            anyhow::bail!("chunk envelope missing chunk count");
        }
        Ok(Self {
            parts: vec![None; count as usize],
            expected: count,
            received: 0,
            expires_at: Instant::now() + FAST_PATH_CHUNK_TTL,
        })
    }

    fn insert(&mut self, index: u32, payload: Vec<u8>) -> anyhow::Result<bool> {
        let idx = index as usize;
        if idx >= self.parts.len() {
            anyhow::bail!("chunk envelope index exceeds count");
        }
        if self.parts[idx].is_none() {
            self.parts[idx] = Some(payload);
            self.received += 1;
        }
        self.expires_at = Instant::now() + FAST_PATH_CHUNK_TTL;
        Ok(self.received == self.expected)
    }

    fn into_payload(self) -> anyhow::Result<Vec<u8>> {
        let mut merged = Vec::new();
        for part in self.parts.into_iter() {
            let chunk = part.ok_or_else(|| anyhow::anyhow!("chunk assembly missing fragment"))?;
            merged.extend_from_slice(&chunk);
        }
        Ok(merged)
    }
}

#[derive(Debug, Deserialize)]
struct FastPathChunkEnvelope {
    #[serde(rename = "type")]
    kind: String,
    scope: String,
    #[serde(default)]
    version: u8,
    id: String,
    index: u32,
    count: u32,
    payload: String,
}

#[derive(Debug)]
pub(crate) struct FastPathChunkReassembler {
    kind: FastPathPayloadKind,
    pending: HashMap<String, PendingChunk>,
}

fn chunk_scope(kind: &FastPathPayloadKind) -> &'static str {
    match kind {
        FastPathPayloadKind::Actions => "actions",
        FastPathPayloadKind::Acks => "acks",
        FastPathPayloadKind::State => "state",
        FastPathPayloadKind::Health => "health",
    }
}

impl FastPathChunkReassembler {
    fn new(kind: FastPathPayloadKind) -> Self {
        Self {
            kind,
            pending: HashMap::new(),
        }
    }

    fn ingest(&mut self, text: &str) -> anyhow::Result<Option<String>> {
        match serde_json::from_str::<FastPathChunkEnvelope>(text) {
            Ok(envelope) => {
                if envelope.kind != "chunk" {
                    return Ok(Some(text.to_string()));
                }
                if envelope.scope != chunk_scope(&self.kind) {
                    anyhow::bail!(
                        "chunk scope mismatch: expected {}, got {}",
                        chunk_scope(&self.kind),
                        envelope.scope
                    );
                }
                if envelope.version != 0 && envelope.version != FAST_PATH_CHUNK_VERSION {
                    anyhow::bail!("unsupported chunk version {}", envelope.version);
                }
                let decoded = BASE64
                    .decode(envelope.payload.as_bytes())
                    .context("decode chunk payload")?;
                let entry = match self.pending.entry(envelope.id.clone()) {
                    Entry::Occupied(existing) => existing.into_mut(),
                    Entry::Vacant(vacant) => vacant.insert(PendingChunk::new(envelope.count)?),
                };
                let complete = entry.insert(envelope.index, decoded)?;
                if complete {
                    let payload = self
                        .pending
                        .remove(&envelope.id)
                        .expect("pending chunk missing")
                        .into_payload()?;
                    let text = String::from_utf8(payload).context("chunk payload utf8 error")?;
                    return Ok(Some(text));
                }
                self.cleanup_expired();
                Ok(None)
            }
            Err(_) => Ok(Some(text.to_string())),
        }
    }

    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.pending.retain(|_, pending| pending.expires_at > now);
    }
}

fn classify_ip_scope(ip: &IpAddr) -> &'static str {
    if is_loopback_ip(ip) {
        "loopback"
    } else if is_private_ip(ip) {
        "private"
    } else if is_link_local_ip(ip) {
        "link_local"
    } else if is_multicast_ip(ip) {
        "multicast"
    } else {
        "public"
    }
}

fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => addr.is_loopback(),
        IpAddr::V6(addr) => addr.is_loopback(),
    }
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => addr.is_private(),
        IpAddr::V6(addr) => addr.is_unique_local(),
    }
}

fn is_link_local_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => addr.is_link_local(),
        IpAddr::V6(addr) => addr.is_unicast_link_local(),
    }
}

fn is_multicast_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => addr.is_multicast(),
        IpAddr::V6(addr) => addr.is_multicast(),
    }
}

fn parse_candidate_meta(candidate: &str) -> Option<IceCandidateMeta> {
    let parts: Vec<_> = candidate.split_whitespace().collect();
    if parts.len() < 6 {
        return None;
    }
    // candidate line: candidate:<id> <component> <protocol> <priority> <ip> <port> typ ...
    let ip_str = parts.get(4)?;
    let port_str = parts.get(5)?;
    let ip = IpAddr::from_str(ip_str).ok()?;
    let port = port_str.parse::<u16>().ok()?;
    let scope = classify_ip_scope(&ip);
    Some(IceCandidateMeta { ip, port, scope })
}

#[derive(Clone)]
pub struct FastPathSession {
    pub session_id: String,
    pub pc: Arc<RTCPeerConnection>,
    pub actions_tx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    pub acks_rx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    pub state_rx: Arc<Mutex<Option<Arc<RTCDataChannel>>>>,
    // local ICE candidates gathered before answer is delivered
    pub local_ice: Arc<RwLock<Vec<serde_json::Value>>>,
    pub public_ip_hint: Option<String>,
    pub host_hint_for_log: Option<String>,
    ack_chunks: Arc<Mutex<FastPathChunkReassembler>>,
    state_chunks: Arc<Mutex<FastPathChunkReassembler>>,
    state: Arc<Mutex<Option<AppState>>>,
    ack_handler_installed: Arc<AtomicBool>,
    state_handler_installed: Arc<AtomicBool>,
    actions_online_notified: Arc<AtomicBool>,
    instance_id: u64,
}

impl FastPathSession {
    pub async fn new(session_id: String) -> Result<Self, WebRtcError> {
        // Build a WebRTC API with optional NAT 1:1 mapping and a fixed UDP port range.
        // This allows the manager (running in Docker) to advertise the host's LAN IP
        // and a published UDP range so external hosts can complete ICE.
        let mut setting = webrtc::api::setting_engine::SettingEngine::default();
        // Force IPv4 transport since the Docker network doesn't provide IPv6 routes.
        // Otherwise the ICE agent keeps trying udp6 candidates and logs noisy warnings.
        setting.set_network_types(vec![NetworkType::Udp4]);

        // Configure ephemeral UDP port range if provided.
        if let (Ok(start_s), Ok(end_s)) = (
            std::env::var("BEACH_ICE_PORT_START"),
            std::env::var("BEACH_ICE_PORT_END"),
        ) {
            if let (Ok(start), Ok(end)) = (start_s.parse::<u16>(), end_s.parse::<u16>()) {
                match EphemeralUDP::new(start, end) {
                    Ok(ephemeral) => setting.set_udp_network(UDPNetwork::Ephemeral(ephemeral)),
                    Err(err) => warn!(
                        target = "fast_path",
                        port_start = start,
                        port_end = end,
                        error = %err,
                        "invalid ICE UDP port range; using defaults"
                    ),
                }
            }
        }

        // Optionally set a NAT 1:1 public IP so the container advertises a host-reachable
        // address (e.g., the Mac's 192.168.x.x) instead of the internal 172.20.x.x.
        let host_hint = std::env::var("BEACH_ICE_PUBLIC_HOST")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let resolved_host = host_hint
            .clone()
            .unwrap_or_else(|| "host.docker.internal".to_string());
        // Prefer explicit IP via BEACH_ICE_PUBLIC_IP; otherwise try resolving a host name
        // (defaulting to host.docker.internal) to an IPv4 address.
        let explicit_public_ip = std::env::var("BEACH_ICE_PUBLIC_IP")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let public_ip = explicit_public_ip.or_else(|| {
            use std::net::ToSocketAddrs;
            // Resolve using an arbitrary port to trigger getaddrinfo.
            (resolved_host.as_str(), 0)
                .to_socket_addrs()
                .ok()
                .and_then(|mut it| it.find(|a| a.is_ipv4()).map(|a| a.ip().to_string()))
        });
        let host_hint_for_log = host_hint.clone().or_else(|| Some(resolved_host.clone()));
        debug!(
            target = "fast_path.ice",
            session_id = %session_id,
            public_ip_hint = public_ip.as_deref(),
            host_hint = host_hint_for_log.as_deref(),
            "configured fast-path ICE hints"
        );
        if let Some(ip) = public_ip.clone() {
            setting.set_nat_1to1_ips(vec![ip], RTCIceCandidateType::Host);
        }

        let api = webrtc::api::APIBuilder::new()
            .with_setting_engine(setting)
            .build();

        let cfg = RTCConfiguration::default();
        let pc = api.new_peer_connection(cfg).await?;

        Ok(FastPathSession {
            session_id,
            pc: Arc::new(pc),
            actions_tx: Arc::new(Mutex::new(None)),
            acks_rx: Arc::new(Mutex::new(None)),
            state_rx: Arc::new(Mutex::new(None)),
            local_ice: Arc::new(RwLock::new(Vec::new())),
            public_ip_hint: public_ip.clone(),
            host_hint_for_log,
            ack_chunks: Arc::new(Mutex::new(FastPathChunkReassembler::new(
                FastPathPayloadKind::Acks,
            ))),
            state_chunks: Arc::new(Mutex::new(FastPathChunkReassembler::new(
                FastPathPayloadKind::State,
            ))),
            state: Arc::new(Mutex::new(None)),
            ack_handler_installed: Arc::new(AtomicBool::new(false)),
            state_handler_installed: Arc::new(AtomicBool::new(false)),
            actions_online_notified: Arc::new(AtomicBool::new(false)),
            instance_id: FAST_PATH_INSTANCE_ID.fetch_add(1, Ordering::SeqCst),
        })
    }

    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// Pre-bind state so early data channel events (on the offer/answer path)
    /// can install handlers without racing the async receiver spawn.
    pub async fn preload_state(&self, state: AppState) {
        self.bind_state(state).await;
    }

    pub async fn set_remote_offer(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, WebRtcError> {
        self.pc.set_remote_description(offer).await?;

        let this = self.clone();
        self.pc
            .on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let label = dc.label().to_string();
                let this2 = this.clone();
                Box::pin(async move {
                    info!(label = %label, "fast-path data channel opened");
                    match label.as_str() {
                        "mgr-actions" => {
                            *this2.actions_tx.lock().await = Some(dc.clone());
                            if let Err(err) = this2.maybe_notify_actions_online().await {
                                warn!(
                                    session_id = %this2.session_id,
                                    error = %err,
                                    "failed to mark actions channel online from data channel event"
                                );
                            }
                        }
                        "mgr-acks" => {
                            *this2.acks_rx.lock().await = Some(dc.clone());
                            if let Err(err) = this2.maybe_install_ack_handler(dc.clone()).await {
                                warn!(
                                    session_id = %this2.session_id,
                                    error = %err,
                                    "failed to install ack handler from data channel event"
                                );
                            }
                        }
                        "mgr-state" => {
                            *this2.state_rx.lock().await = Some(dc.clone());
                            if let Err(err) = this2.maybe_install_state_handler(dc.clone()).await {
                                warn!(
                                    session_id = %this2.session_id,
                                    error = %err,
                                    "failed to install state handler from data channel event"
                                );
                            }
                        }
                        _ => {}
                    }
                })
            }));

        let this = self.clone();
        self.pc.on_ice_candidate(Box::new(move |c| {
            let this3 = this.clone();
            Box::pin(async move {
                if let Some(cand) = c {
                    if let Ok(json) = cand.to_json() {
                        let val = serde_json::json!({
                            "candidate": json.candidate,
                            "sdp_mid": json.sdp_mid,
                            "sdp_mline_index": json.sdp_mline_index,
                        });
                        this3.local_ice.write().await.push(val);
                        let candidate_meta = parse_candidate_meta(&json.candidate);
                        if should_log_custom_event(
                            "fast_path_local_candidate",
                            &this3.session_id,
                            Duration::from_secs(30),
                        ) {
                            debug!(
                                target = "fast_path.ice",
                                session_id = %this3.session_id,
                                candidate = %json.candidate,
                                candidate_ip = candidate_meta.as_ref().map(|meta| meta.ip.to_string()),
                                candidate_port = candidate_meta.as_ref().map(|meta| meta.port),
                                candidate_scope = candidate_meta.as_ref().map(|meta| meta.scope),
                                "local ICE candidate gathered"
                            );
                        }
                        if let Some(meta) = candidate_meta {
                            if meta.scope == "loopback" {
                                warn!(
                                    target = "fast_path.ice",
                                    session_id = %this3.session_id,
                                    ip = %meta.ip,
                                    port = meta.port,
                                    "local ICE candidate is loopback; hosts outside the container cannot reach it"
                                );
                            }
                        }
                    }
                }
            })
        }));

        let ice_ip_hint = self.public_ip_hint.clone();
        let ice_host_hint = self.host_hint_for_log.clone();
        let session_for_state = self.session_id.clone();
        self.pc
            .on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
                let session = session_for_state.clone();
                let ip_hint = ice_ip_hint.clone();
                let host_hint = ice_host_hint.clone();
                Box::pin(async move {
                    debug!(
                        target = "fast_path.ice",
                        session_id = %session,
                        ?state,
                        public_ip_hint = ip_hint.as_deref(),
                        host_hint = host_hint.as_deref(),
                        "ice connection state change"
                    );
                    if state == RTCIceConnectionState::Failed {
                        warn!(
                            target = "fast_path.ice",
                            session_id = %session,
                            public_ip_hint = ip_hint.as_deref(),
                            host_hint = host_hint.as_deref(),
                            "ice connection reported failure"
                        );
                    }
                })
            }));

        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer.clone()).await?;
        Ok(answer)
    }

    pub async fn add_remote_ice(&self, cand: RTCIceCandidateInit) -> Result<(), WebRtcError> {
        let candidate_meta = parse_candidate_meta(&cand.candidate);
        if should_log_custom_event(
            "fast_path_remote_candidate",
            &self.session_id,
            Duration::from_secs(30),
        ) {
            debug!(
                target = "fast_path.ice",
                session_id = %self.session_id,
                candidate = %cand.candidate,
                candidate_ip = candidate_meta.as_ref().map(|meta| meta.ip.to_string()),
                candidate_port = candidate_meta.as_ref().map(|meta| meta.port),
                candidate_scope = candidate_meta.as_ref().map(|meta| meta.scope),
                "applying remote ICE candidate"
            );
        }
        if let Some(meta) = candidate_meta {
            if meta.scope == "loopback" {
                warn!(
                    target = "fast_path.ice",
                    session_id = %self.session_id,
                    ip = %meta.ip,
                    port = meta.port,
                    "remote ICE candidate is loopback; external hosts will be unreachable"
                );
            }
        }
        self.pc.add_ice_candidate(cand).await
    }

    pub async fn local_description(&self) -> Option<RTCSessionDescription> {
        self.pc.local_description().await
    }

    pub fn spawn_receivers(self: &Arc<Self>, state: AppState) {
        let _ = self.bind_state(state.clone());
        let ack_session = Arc::clone(self);
        let ack_state = state.clone();
        tokio::spawn(async move {
            if let Some(dc) = wait_for_channel(ack_session.clone(), ChannelKind::Acks).await {
                if let Err(err) = ack_session.maybe_install_ack_handler(dc.clone()).await {
                    warn!(
                        session_id = %ack_session.session_id,
                        error = %err,
                        "fast-path ack handler install failed; falling back to legacy path"
                    );
                    install_ack_handler(dc, ack_session.clone(), ack_state.clone());
                }
            } else {
                warn!(
                    session_id = %ack_session.session_id,
                    "fast-path ack channel not established; continuing with HTTP fallback"
                );
            }
        });

        let state_session = Arc::clone(self);
        let state_state = state.clone();
        tokio::spawn(async move {
            if let Some(dc) = wait_for_channel(state_session.clone(), ChannelKind::State).await {
                if let Err(err) = state_session.maybe_install_state_handler(dc.clone()).await {
                    warn!(
                        session_id = %state_session.session_id,
                        error = %err,
                        "fast-path state handler install failed; falling back to legacy path"
                    );
                    install_state_handler(dc, state_session.clone(), state_state.clone());
                }
            } else {
                warn!(
                    session_id = %state_session.session_id,
                    "fast-path state channel not established; continuing with HTTP fallback"
                );
            }
        });

        let actions_session = Arc::clone(self);
        let actions_state = state.clone();
        tokio::spawn(async move {
            if wait_for_channel(actions_session.clone(), ChannelKind::Actions)
                .await
                .is_some()
            {
                if let Err(err) = actions_session.maybe_notify_actions_online().await {
                    warn!(
                        session_id = %actions_session.session_id,
                        error = %err,
                        "failed to mark fast-path actions online; clearing backlog via fallback"
                    );
                    actions_state
                        .fast_path_actions_online(&actions_session.session_id)
                        .await;
                }
            } else {
                warn!(
                    session_id = %actions_session.session_id,
                    "fast-path actions channel not established; continuing with HTTP fallback"
                );
            }
        });
    }

    async fn clear_ack_channel(&self) {
        *self.acks_rx.lock().await = None;
    }

    async fn clear_state_channel(&self) {
        *self.state_rx.lock().await = None;
    }

    async fn bind_state(&self, state: AppState) {
        let mut guard = self.state.lock().await;
        *guard = Some(state);
    }

    async fn maybe_install_ack_handler(
        &self,
        dc: Arc<RTCDataChannel>,
    ) -> Result<(), anyhow::Error> {
        if self
            .ack_handler_installed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let guard = self.state.lock().await;
        let Some(state) = guard.as_ref() else {
            self.ack_handler_installed.store(false, Ordering::SeqCst);
            anyhow::bail!("fast-path state handle missing");
        };
        install_ack_handler(dc, Arc::new(self.clone()), state.clone());
        Ok(())
    }

    async fn maybe_install_state_handler(
        &self,
        dc: Arc<RTCDataChannel>,
    ) -> Result<(), anyhow::Error> {
        if self
            .state_handler_installed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let guard = self.state.lock().await;
        let Some(state) = guard.as_ref() else {
            self.state_handler_installed.store(false, Ordering::SeqCst);
            anyhow::bail!("fast-path state handle missing");
        };
        install_state_handler(dc, Arc::new(self.clone()), state.clone());
        Ok(())
    }

    async fn maybe_notify_actions_online(&self) -> Result<(), anyhow::Error> {
        if self
            .actions_online_notified
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let guard = self.state.lock().await;
        let Some(state) = guard.as_ref() else {
            self.actions_online_notified.store(false, Ordering::SeqCst);
            anyhow::bail!("fast-path state handle missing");
        };
        state.fast_path_actions_online(&self.session_id).await;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct FastPathRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<FastPathSession>>>>,
}

impl FastPathRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, session_id: String, fps: Arc<FastPathSession>) {
        self.inner.write().await.insert(session_id, fps);
    }

    pub async fn get(&self, session_id: &str) -> Option<Arc<FastPathSession>> {
        self.inner.read().await.get(session_id).cloned()
    }

    pub async fn remove(&self, session_id: &str) -> Option<Arc<FastPathSession>> {
        self.inner.write().await.remove(session_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastPathSendOutcome {
    Delivered,
    SessionMissing,
    ChannelMissing,
}

pub async fn send_actions_over_fast_path(
    registry: &FastPathRegistry,
    session_id: &str,
    actions: &[ActionCommand],
) -> anyhow::Result<FastPathSendOutcome> {
    // Bound the time spent on fast-path sends so controller callers are not
    // indefinitely blocked if the data channel stalls. When the timeout
    // elapses, the caller will fall back to HTTP/Redis delivery.
    const FAST_PATH_SEND_TIMEOUT_MS: u64 = 1000;
    const FAST_PATH_LOG_INTERVAL: Duration = Duration::from_secs(5);

    if let Some(fps) = registry.get(session_id).await {
        let guard = fps.actions_tx.lock().await;
        if let Some(dc) = guard.as_ref() {
            for a in actions {
                let payload = fast_path_action_payload(a).map_err(anyhow::Error::msg)?;
                let timeout_duration = Duration::from_millis(FAST_PATH_SEND_TIMEOUT_MS);
                let send_result = timeout(timeout_duration, dc.send_text(&payload)).await;
                match send_result {
                    Ok(Ok(_)) => {
                        // Normal fast-path delivery.
                    }
                    Ok(Err(err)) => {
                        debug!(
                            target = "fast_path",
                            session_id = %session_id,
                            error = %err,
                            "fast-path send failed; propagating error to caller"
                        );
                        return Err(anyhow::anyhow!(err.to_string()));
                    }
                    Err(_) => {
                        let message = format!(
                            "fast-path send timed out after {}ms",
                            FAST_PATH_SEND_TIMEOUT_MS
                        );
                        debug!(
                            target = "fast_path",
                            session_id = %session_id,
                            "{}", message
                        );
                        return Err(anyhow::anyhow!(message));
                    }
                }
            }
            debug!(
                target = "fast_path.delivery",
                session_id = %session_id,
                action_count = actions.len(),
                "fast-path actions delivered"
            );
            return Ok(FastPathSendOutcome::Delivered);
        } else if should_log_custom_event(
            "fast_path_action_channel_missing",
            session_id,
            FAST_PATH_LOG_INTERVAL,
        ) {
            trace!(
                target = "fast_path",
                session_id = %session_id,
                action_count = actions.len(),
                "fast-path actions channel not ready; falling back to HTTP"
            );
        }
        return Ok(FastPathSendOutcome::ChannelMissing);
    } else if should_log_custom_event(
        "fast_path_session_missing",
        session_id,
        FAST_PATH_LOG_INTERVAL,
    ) {
        trace!(
            target = "fast_path",
            session_id = %session_id,
            action_count = actions.len(),
            "fast-path session not registered; falling back to HTTP"
        );
    }
    Ok(FastPathSendOutcome::SessionMissing)
}

async fn wait_for_channel(
    session: Arc<FastPathSession>,
    kind: ChannelKind,
) -> Option<Arc<RTCDataChannel>> {
    const MAX_ATTEMPTS: usize = 40;
    const INTERVAL: Duration = Duration::from_millis(50);

    for attempt in 0..MAX_ATTEMPTS {
        let maybe = match kind {
            ChannelKind::Actions => session.actions_tx.lock().await.clone(),
            ChannelKind::Acks => session.acks_rx.lock().await.clone(),
            ChannelKind::State => session.state_rx.lock().await.clone(),
        };
        if let Some(dc) = maybe {
            info!(
                session_id = %session.session_id,
                channel = %dc.label(),
                attempt,
                fast_path_id = session.instance_id,
                "fast-path data channel ready"
            );
            return Some(dc);
        }
        sleep(INTERVAL).await;
    }
    None
}

fn install_ack_handler(dc: Arc<RTCDataChannel>, session: Arc<FastPathSession>, state: AppState) {
    let session_id = session.session_id.clone();
    let state_clone = state.clone();
    let state_for_close = state.clone();
    let state_for_error = state.clone();
    let session_for_message = session.clone();
    let session_for_close = session.clone();
    let session_for_error = session;
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let state = state_clone.clone();
        let session_id = session_id.clone();
        let session = session_for_message.clone();
        Box::pin(async move {
            match parse_action_ack(&msg, &session.ack_chunks).await {
                Ok(Some(ack)) => {
                    if let Err(err) = state.ack_actions(&session_id, vec![ack], None, true).await {
                        warn!(
                            target = "fast_path",
                            session_id = %session_id,
                            error = %err,
                            "failed to persist ack from fast-path"
                        );
                    }
                }
                Ok(None) => {
                    trace!(
                        target = "fast_path",
                        session_id = %session_id,
                        "received partial fast-path ack chunk; waiting for completion"
                    );
                }
                Err(err) => {
                    warn!(
                        target = "fast_path",
                        session_id = %session_id,
                        error = %err,
                        "failed to parse ack message from fast-path channel"
                    );
                    if let Some((pb, sess)) = state.session_metrics_labels(&session_id).await {
                        metrics::FASTPATH_CHANNEL_ERRORS
                            .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-acks"])
                            .inc();
                    }
                }
            }
        })
    }));

    dc.on_close(Box::new(move || {
        let session = session_for_close.clone();
        let state = state_for_close.clone();
        Box::pin(async move {
            session.clear_ack_channel().await;
            info!(
                session_id = %session.session_id,
                "fast-path ack channel closed"
            );
            if let Some((pb, sess)) = state.session_metrics_labels(&session.session_id).await {
                metrics::FASTPATH_CHANNEL_CLOSED
                    .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-acks"])
                    .inc();
            }
        })
    }));

    dc.on_error(Box::new(move |err| {
        let session = session_for_error.clone();
        let state = state_for_error.clone();
        Box::pin(async move {
            warn!(
                target = "fast_path",
                session_id = %session.session_id,
                error = %err,
                "fast-path ack channel error"
            );
            if let Some((pb, sess)) = state.session_metrics_labels(&session.session_id).await {
                metrics::FASTPATH_CHANNEL_ERRORS
                    .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-acks"])
                    .inc();
            }
        })
    }));
}

fn install_state_handler(dc: Arc<RTCDataChannel>, session: Arc<FastPathSession>, state: AppState) {
    let session_id = session.session_id.clone();
    let state_clone = state.clone();
    let state_for_close = state.clone();
    let state_for_error = state.clone();
    let session_for_message = session.clone();
    let session_for_close = session.clone();
    let session_for_error = session;
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let state = state_clone.clone();
        let session_id = session_id.clone();
        let session = session_for_message.clone();
        Box::pin(async move {
            match parse_state_diff(&msg, &session.state_chunks).await {
                Ok(Some(diff)) => {
                    if let Err(err) = state.record_state(&session_id, diff, true).await {
                        warn!(
                            target = "fast_path",
                            session_id = %session_id,
                            error = %err,
                            "failed to persist state diff from fast-path"
                        );
                    }
                }
                Ok(None) => {
                    trace!(
                        target = "fast_path",
                        session_id = %session_id,
                        "received partial fast-path state chunk; waiting for completion"
                    );
                }
                Err(err) => {
                    warn!(
                        target = "fast_path",
                        session_id = %session_id,
                        error = %err,
                        "failed to parse state message from fast-path channel"
                    );
                    if let Some((pb, sess)) = state.session_metrics_labels(&session_id).await {
                        metrics::FASTPATH_CHANNEL_ERRORS
                            .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-state"])
                            .inc();
                    }
                }
            }
        })
    }));

    dc.on_close(Box::new(move || {
        let session = session_for_close.clone();
        let state = state_for_close.clone();
        Box::pin(async move {
            session.clear_state_channel().await;
            info!(
                session_id = %session.session_id,
                "fast-path state channel closed"
            );
            if let Some((pb, sess)) = state.session_metrics_labels(&session.session_id).await {
                metrics::FASTPATH_CHANNEL_CLOSED
                    .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-state"])
                    .inc();
            }
        })
    }));

    dc.on_error(Box::new(move |err| {
        let session = session_for_error.clone();
        let state = state_for_error.clone();
        Box::pin(async move {
            warn!(
                target = "fast_path",
                session_id = %session.session_id,
                error = %err,
                "fast-path state channel error"
            );
            if let Some((pb, sess)) = state.session_metrics_labels(&session.session_id).await {
                metrics::FASTPATH_CHANNEL_ERRORS
                    .with_label_values(&[pb.as_str(), sess.as_str(), "mgr-state"])
                    .inc();
            }
        })
    }));
}

async fn assemble_fast_path_payload(
    msg: &DataChannelMessage,
    chunks: &Arc<Mutex<FastPathChunkReassembler>>,
) -> anyhow::Result<Option<String>> {
    if !msg.is_string {
        anyhow::bail!("expected text payload");
    }
    let text = String::from_utf8(msg.data.to_vec())?;
    let mut guard = chunks.lock().await;
    guard.ingest(&text)
}

async fn parse_action_ack(
    msg: &DataChannelMessage,
    chunks: &Arc<Mutex<FastPathChunkReassembler>>,
) -> anyhow::Result<Option<ActionAck>> {
    let Some(text) = assemble_fast_path_payload(msg, chunks).await? else {
        return Ok(None);
    };
    let envelope: AckEnvelope = serde_json::from_str(&text)?;
    if envelope.kind != "ack" {
        anyhow::bail!("unexpected message type {}", envelope.kind);
    }
    Ok(Some(envelope.payload))
}

async fn parse_state_diff(
    msg: &DataChannelMessage,
    chunks: &Arc<Mutex<FastPathChunkReassembler>>,
) -> anyhow::Result<Option<StateDiff>> {
    let Some(text) = assemble_fast_path_payload(msg, chunks).await? else {
        return Ok(None);
    };
    let envelope: StateEnvelope = serde_json::from_str(&text)?;
    if envelope.kind != "state" {
        anyhow::bail!("unexpected message type {}", envelope.kind);
    }
    Ok(Some(envelope.payload))
}

pub fn fast_path_action_payload(action: &ActionCommand) -> Result<String, String> {
    // Preserve the JSON envelope expected by the host-side fast-path consumer.
    #[derive(Serialize)]
    struct ActionEnvelope<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        payload: &'a ActionCommand,
    }

    serde_json::to_string(&ActionEnvelope {
        kind: "action",
        payload: action,
    })
    .map_err(|err| err.to_string())
}

pub fn action_terminal_bytes(action: &ActionCommand) -> Result<Vec<u8>, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    let bytes = action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())?;
    Ok(bytes.as_bytes().to_vec())
}

#[derive(Deserialize, Serialize)]
struct AckEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: ActionAck,
}

#[derive(Deserialize, Serialize)]
struct StateEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: StateDiff,
}

enum ChannelKind {
    Actions,
    Acks,
    State,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde_json::json;

    #[tokio::test]
    async fn parses_ack_envelope() {
        let base_ack = ActionAck {
            id: "a1".into(),
            status: beach_buggy::AckStatus::Ok,
            applied_at: std::time::SystemTime::now(),
            latency_ms: Some(5),
            error_code: None,
            error_message: None,
        };
        let chunks = Arc::new(Mutex::new(FastPathChunkReassembler::new(
            FastPathPayloadKind::Acks,
        )));
        let msg = DataChannelMessage {
            is_string: true,
            data: Bytes::from(
                serde_json::to_string(&AckEnvelope {
                    kind: "ack".into(),
                    payload: base_ack.clone(),
                })
                .unwrap(),
            ),
        };
        let ack = parse_action_ack(&msg, &chunks)
            .await
            .expect("parsed ack")
            .expect("complete ack payload");
        assert_eq!(ack.id, "a1");
    }

    #[tokio::test]
    async fn parses_state_envelope() {
        let base_diff = StateDiff {
            sequence: 7,
            emitted_at: std::time::SystemTime::now(),
            payload: serde_json::json!({"ops": []}),
        };
        let chunks = Arc::new(Mutex::new(FastPathChunkReassembler::new(
            FastPathPayloadKind::State,
        )));
        let msg = DataChannelMessage {
            is_string: true,
            data: Bytes::from(
                serde_json::to_string(&StateEnvelope {
                    kind: "state".into(),
                    payload: base_diff.clone(),
                })
                .unwrap(),
            ),
        };
        let diff = parse_state_diff(&msg, &chunks)
            .await
            .expect("parsed state")
            .expect("complete state payload");
        assert_eq!(diff.sequence, 7);
    }

    #[tokio::test]
    async fn reassembles_chunked_state_envelope() {
        let base_diff = StateDiff {
            sequence: 9,
            emitted_at: std::time::SystemTime::now(),
            payload: serde_json::json!({"ops": [{"insert": "x".repeat(40_000)}]}),
        };
        let payload = serde_json::to_string(&StateEnvelope {
            kind: "state".into(),
            payload: base_diff.clone(),
        })
        .unwrap();
        let frames =
            beach_buggy::fast_path::frame_fast_path_payload(FastPathPayloadKind::State, &payload)
                .expect("framed payload");
        assert!(frames.len() > 1, "chunking should produce multiple frames");

        let chunks = Arc::new(Mutex::new(FastPathChunkReassembler::new(
            FastPathPayloadKind::State,
        )));
        for (idx, frame) in frames.iter().enumerate() {
            let msg = DataChannelMessage {
                is_string: true,
                data: Bytes::from(frame.clone()),
            };
            let parsed = parse_state_diff(&msg, &chunks)
                .await
                .expect("parse attempt");
            if idx < frames.len() - 1 {
                assert!(
                    parsed.is_none(),
                    "intermediate chunk should not yield payload"
                );
            } else {
                let diff = parsed.expect("final chunk delivers payload");
                assert_eq!(diff.sequence, base_diff.sequence);
            }
        }
    }

    #[test]
    fn encodes_action_payload_as_text_envelope() {
        let action = ActionCommand {
            id: "a1".into(),
            action_type: "terminal_write".into(),
            payload: json!({ "bytes": "hello\n" }),
            expires_at: None,
        };
        let text = fast_path_action_payload(&action).expect("payload");
        let parsed =
            beach_buggy::fast_path::parse_action_payload(&text).expect("action parsed by client");
        assert_eq!(parsed.id, action.id);
        assert_eq!(parsed.action_type, action.action_type);
        assert_eq!(parsed.payload, action.payload);
    }
}

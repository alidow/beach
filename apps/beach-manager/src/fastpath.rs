use std::{
    collections::HashMap,
    net::IpAddr,
    str::FromStr,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use beach_buggy::{
    fast_path::{frame_fast_path_payload, FastPathChunkReassembler, FastPathPayloadKind},
    ActionAck, ActionCommand, StateDiff,
};
use beach_client_core::protocol::{self, ClientFrame as WireClientFrame};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, RwLock},
    time::{sleep, timeout, Duration, Instant},
};
use tracing::{debug, info, trace, warn};

use webrtc::data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::Error as WebRtcError;
use webrtc_ice::{
    network_type::NetworkType,
    udp_network::{EphemeralUDP, UDPNetwork},
};

use crate::{log_throttle::should_log_custom_event, metrics, state::AppState};

#[derive(Debug)]
struct IceCandidateMeta {
    ip: IpAddr,
    port: u16,
    scope: &'static str,
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

#[derive(Debug, Deserialize)]
struct IceServerOverride {
    urls: Vec<String>,
    username: Option<String>,
    credential: Option<String>,
}

fn default_manager_ice_servers() -> Vec<RTCIceServer> {
    let mut servers = Vec::new();
    servers.push(RTCIceServer {
        urls: vec!["stun:host.docker.internal:3478".to_string()],
        ..Default::default()
    });
    servers.push(RTCIceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        ..Default::default()
    });
    servers
}

fn raw_ice_override_env() -> Option<(String, &'static str)> {
    if let Ok(value) = std::env::var("BEACH_MANAGER_ICE_SERVERS") {
        return Some((value, "BEACH_MANAGER_ICE_SERVERS"));
    }
    if let Ok(value) = std::env::var("BEACH_ICE_SERVERS") {
        return Some((value, "BEACH_ICE_SERVERS"));
    }
    None
}

fn manager_ice_servers_from_env() -> Option<(Vec<RTCIceServer>, &'static str)> {
    let (raw, env_name) = raw_ice_override_env()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let overrides: Vec<IceServerOverride> = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                target = "fast_path.ice",
                error = %err,
                env = env_name,
                "failed to parse ICE override"
            );
            return None;
        }
    };

    let mut servers = Vec::new();
    for server in overrides.into_iter() {
        if server.urls.is_empty() {
            continue;
        }
        servers.push(RTCIceServer {
            urls: server.urls,
            username: server.username.unwrap_or_default(),
            credential: server.credential.unwrap_or_default(),
            ..Default::default()
        });
    }

    if servers.is_empty() {
        warn!(
            target = "fast_path.ice",
            env = env_name,
            "ICE override did not include usable urls"
        );
        return None;
    }

    info!(
        target = "fast_path.ice",
        env = env_name,
        server_count = servers.len(),
        "using ICE servers from override env"
    );
    Some((servers, env_name))
}

fn resolve_manager_ice_servers() -> (Vec<RTCIceServer>, &'static str) {
    if let Some((servers, env)) = manager_ice_servers_from_env() {
        return (servers, env);
    }

    (default_manager_ice_servers(), "default_public_stun")
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
    next_seq: Arc<AtomicU64>,
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
        let port_start_env = std::env::var("BEACH_ICE_PORT_START").ok();
        let port_end_env = std::env::var("BEACH_ICE_PORT_END").ok();
        if let (Some(start_s), Some(end_s)) = (port_start_env.as_deref(), port_end_env.as_deref()) {
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
        // Prefer explicit IP via BEACH_ICE_PUBLIC_IP; otherwise resolve BEACH_ICE_PUBLIC_HOST.
        let explicit_public_ip = std::env::var("BEACH_ICE_PUBLIC_IP")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let nat_ip = explicit_public_ip.or_else(|| {
            host_hint.as_deref().and_then(|host| {
                use std::net::ToSocketAddrs;
                (host, 0)
                    .to_socket_addrs()
                    .ok()
                    .and_then(|mut it| it.find(|a| a.is_ipv4()).map(|a| a.ip().to_string()))
            })
        });
        let host_hint_for_log = host_hint.clone();
        debug!(
            target = "fast_path.ice",
            session_id = %session_id,
            public_ip_hint = nat_ip.as_deref(),
            host_hint = host_hint_for_log.as_deref(),
            port_start = port_start_env.as_deref(),
            port_end = port_end_env.as_deref(),
            nat_hint_enabled = nat_ip.is_some(),
            "configured fast-path ICE hints"
        );
        if let Some(ip) = nat_ip.clone() {
            setting.set_nat_1to1_ips(vec![ip], RTCIceCandidateType::Host);
        }

        let (ice_servers, ice_source) = resolve_manager_ice_servers();
        debug!(
            target = "fast_path.ice",
            session_id = %session_id,
            ice_source,
            server_count = ice_servers.len(),
            "configured manager ICE servers"
        );

        let api = webrtc::api::APIBuilder::new()
            .with_setting_engine(setting)
            .build();

        let mut cfg = RTCConfiguration::default();
        cfg.ice_servers = ice_servers;
        let pc = api.new_peer_connection(cfg).await?;

        Ok(FastPathSession {
            session_id,
            pc: Arc::new(pc),
            actions_tx: Arc::new(Mutex::new(None)),
            acks_rx: Arc::new(Mutex::new(None)),
            state_rx: Arc::new(Mutex::new(None)),
            local_ice: Arc::new(RwLock::new(Vec::new())),
            public_ip_hint: nat_ip.clone(),
            host_hint_for_log,
            next_seq: Arc::new(AtomicU64::new(1)),
        })
    }

    fn next_sequence(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::SeqCst)
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
                        }
                        "mgr-acks" => {
                            *this2.acks_rx.lock().await = Some(dc.clone());
                        }
                        "mgr-state" => {
                            *this2.state_rx.lock().await = Some(dc.clone());
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
                        trace!(
                            target = "fast_path.ice",
                            session_id = %this3.session_id,
                            candidate = %json.candidate,
                            sdp_mid = json.sdp_mid.as_deref(),
                            sdp_mline_index = json.sdp_mline_index,
                            "local ICE candidate gathered"
                        );
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
        let local_candidates_for_state = Arc::clone(&self.local_ice);
        let pc_for_close = Arc::clone(&self.pc);
        self.pc
            .on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
                let session = session_for_state.clone();
                let ip_hint = ice_ip_hint.clone();
                let host_hint = ice_host_hint.clone();
                let candidate_snapshot = Arc::clone(&local_candidates_for_state);
                let pc_close = Arc::clone(&pc_for_close);
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
                        let (recent_candidates, total_candidates) = {
                            let guard = candidate_snapshot.read().await;
                            let total = guard.len();
                            let mut preview = Vec::new();
                            for value in guard.iter().rev() {
                                if let Some(candidate) =
                                    value.get("candidate").and_then(|v| v.as_str())
                                {
                                    if let Some(meta) = parse_candidate_meta(candidate) {
                                        preview.push(format!(
                                            "{}:{} ({})",
                                            meta.ip, meta.port, meta.scope
                                        ));
                                    }
                                }
                                if preview.len() >= 5 {
                                    break;
                                }
                            }
                            (preview, total)
                        };
                        warn!(
                            target = "fast_path.ice",
                            session_id = %session,
                            public_ip_hint = ip_hint.as_deref(),
                            host_hint = host_hint.as_deref(),
                            candidate_count = total_candidates,
                            recent_candidates = ?recent_candidates,
                            "ice connection reported failure"
                        );
                        // Stop the underlying agent so it does not spin forever trying to
                        // ping unreachable candidates (which also spams warnings/CPU).
                        if let Err(err) = pc_close.close().await {
                            trace!(
                                target = "fast_path.ice",
                                session_id = %session,
                                error = %err,
                                "failed to close peer connection after ICE failure"
                            );
                        }
                    }
                })
            }));

        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer.clone()).await?;
        Ok(answer)
    }

    pub async fn add_remote_ice(&self, cand: RTCIceCandidateInit) -> Result<(), WebRtcError> {
        trace!(
            target = "fast_path.ice",
            session_id = %self.session_id,
            candidate = %cand.candidate,
            sdp_mid = cand.sdp_mid.as_deref(),
            sdp_mline_index = cand.sdp_mline_index,
            "received remote ICE candidate"
        );
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
        let ack_session = Arc::clone(self);
        let ack_state = state.clone();
        let state_channel_state = state.clone();
        let actions_state = state;
        tokio::spawn(async move {
            if let Some(dc) = wait_for_channel(ack_session.clone(), ChannelKind::Acks).await {
                install_ack_handler(dc, ack_session.clone(), ack_state.clone());
            } else {
                warn!(
                    session_id = %ack_session.session_id,
                    "fast-path ack channel not established; continuing with HTTP fallback"
                );
            }
        });

        let state_session = Arc::clone(self);
        tokio::spawn(async move {
            if let Some(dc) = wait_for_channel(state_session.clone(), ChannelKind::State).await {
                install_state_handler(dc, state_session.clone(), state_channel_state.clone());
            } else {
                warn!(
                    session_id = %state_session.session_id,
                    "fast-path state channel not established; continuing with HTTP fallback"
                );
            }
        });

        let actions_session = Arc::clone(self);
        tokio::spawn(async move {
            if wait_for_channel(actions_session.clone(), ChannelKind::Actions)
                .await
                .is_some()
            {
                actions_state
                    .fast_path_actions_online(&actions_session.session_id)
                    .await;
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
                let bytes = fast_path_action_bytes(a).map_err(anyhow::Error::msg)?;
                let seq = fps.next_sequence();
                let frame = WireClientFrame::Input {
                    seq,
                    data: bytes,
                };
                let encoded = protocol::encode_client_frame_binary(&frame);
                let timeout_duration = Duration::from_millis(FAST_PATH_SEND_TIMEOUT_MS);
                let send_result = timeout(timeout_duration, dc.send(&Bytes::from(encoded))).await;
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
    wait_for_channel_with_timeout(|| async {
        match kind {
            ChannelKind::Actions => session.actions_tx.lock().await.clone(),
            ChannelKind::Acks => session.acks_rx.lock().await.clone(),
            ChannelKind::State => session.state_rx.lock().await.clone(),
        }
    })
    .await
}

const FAST_PATH_CHANNEL_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const FAST_PATH_CHANNEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

async fn wait_for_channel_with_timeout<F, Fut, T>(
    mut fetch: F,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + FAST_PATH_CHANNEL_WAIT_TIMEOUT;
    loop {
        if let Some(value) = fetch().await {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(FAST_PATH_CHANNEL_POLL_INTERVAL).await;
    }
}

fn install_ack_handler(dc: Arc<RTCDataChannel>, session: Arc<FastPathSession>, state: AppState) {
    let session_id = session.session_id.clone();
    let state_clone = state.clone();
    let state_for_close = state.clone();
    let state_for_error = state.clone();
    let reassembler = Arc::new(Mutex::new(FastPathChunkReassembler::new(
        FastPathPayloadKind::Acks,
    )));
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let state = state_clone.clone();
        let session_id = session_id.clone();
        let reassembler = reassembler.clone();
        Box::pin(async move {
            match decode_chunked_text(&reassembler, &msg).await {
                Ok(Some(text)) => match parse_action_ack(&text) {
                    Ok(ack) => {
                        if let Err(err) =
                            state.ack_actions(&session_id, vec![ack], None, true).await
                        {
                            warn!(
                                target = "fast_path",
                                session_id = %session_id,
                                error = %err,
                                "failed to persist ack from fast-path"
                            );
                        }
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
                },
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        target = "fast_path",
                        session_id = %session_id,
                        error = %err,
                        "failed to decode chunked ack message"
                    );
                }
            }
        })
    }));

    let session_close = session.clone();
    dc.on_close(Box::new(move || {
        let session = session_close.clone();
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

    let session_error = session.clone();
    dc.on_error(Box::new(move |err| {
        let session = session_error.clone();
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
    let reassembler = Arc::new(Mutex::new(FastPathChunkReassembler::new(
        FastPathPayloadKind::State,
    )));
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let state = state_clone.clone();
        let session_id = session_id.clone();
        let reassembler = reassembler.clone();
        Box::pin(async move {
            match decode_chunked_text(&reassembler, &msg).await {
                Ok(Some(text)) => match parse_state_diff(&text) {
                    Ok(diff) => {
                        if let Err(err) = state.record_state(&session_id, diff, true).await {
                            warn!(
                                target = "fast_path",
                                session_id = %session_id,
                                error = %err,
                                "failed to persist state diff from fast-path"
                            );
                        }
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
                },
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        target = "fast_path",
                        session_id = %session_id,
                        error = %err,
                        "failed to decode chunked state message"
                    );
                }
            }
        })
    }));

    let session_close = session.clone();
    dc.on_close(Box::new(move || {
        let session = session_close.clone();
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

    let session_error = session.clone();
    dc.on_error(Box::new(move |err| {
        let session = session_error.clone();
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

async fn decode_chunked_text(
    reassembler: &Arc<Mutex<FastPathChunkReassembler>>,
    msg: &DataChannelMessage,
) -> anyhow::Result<Option<String>> {
    if !msg.is_string {
        anyhow::bail!("expected text payload");
    }
    let text = String::from_utf8(msg.data.to_vec())?;
    let mut guard = reassembler.lock().await;
    guard
        .ingest(&text)
        .map_err(|err| anyhow::anyhow!(err.to_string()))
}

fn parse_action_ack(text: &str) -> anyhow::Result<ActionAck> {
    let envelope: AckEnvelope = serde_json::from_str(text)?;
    if envelope.kind != "ack" {
        anyhow::bail!("unexpected message type {}", envelope.kind);
    }
    Ok(envelope.payload)
}

fn parse_state_diff(text: &str) -> anyhow::Result<StateDiff> {
    let envelope: StateEnvelope = serde_json::from_str(text)?;
    if envelope.kind != "state" {
        anyhow::bail!("unexpected message type {}", envelope.kind);
    }
    Ok(envelope.payload)
}

fn terminal_write_bytes(action: &ActionCommand) -> Result<&str, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())
}

pub fn fast_path_action_bytes(action: &ActionCommand) -> Result<Vec<u8>, String> {
    let bytes = terminal_write_bytes(action)?;
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
    use std::sync::Arc;
    use tokio::runtime::Runtime;
    use tokio::sync::Mutex;
    use tokio::time;

    #[tokio::test(start_paused = true)]
    async fn wait_for_channel_with_timeout_respects_extended_deadline() {
        let attempts = Arc::new(Mutex::new(0usize));
        let fut = wait_for_channel_with_timeout({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    *attempts.lock().await += 1;
                    None::<Arc<()>>
                }
            }
        });
        tokio::pin!(fut);
        time::advance(FAST_PATH_CHANNEL_WAIT_TIMEOUT).await;
        assert!(fut.await.is_none(), "channel lookup should respect timeout");
        let attempt_count = *attempts.lock().await;
        let expected = FAST_PATH_CHANNEL_WAIT_TIMEOUT.as_millis()
            / FAST_PATH_CHANNEL_POLL_INTERVAL.as_millis()
            + 1;
        assert!(attempt_count as u128 >= expected, "attempts should continue until timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_channel_with_timeout_returns_value_once_available() {
        let attempts = Arc::new(Mutex::new(0usize));
        let fut = wait_for_channel_with_timeout({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    let mut guard = attempts.lock().await;
                    *guard += 1;
                    if *guard == 5 {
                        Some("ready".to_string())
                    } else {
                        None
                    }
                }
            }
        });
        tokio::pin!(fut);
        let advance = FAST_PATH_CHANNEL_POLL_INTERVAL * 5;
        time::advance(advance).await;
        assert_eq!(fut.await, Some("ready".to_string()));
    }

    #[test]
    fn parses_ack_envelope() {
        let base_ack = ActionAck {
            id: "a1".into(),
            status: beach_buggy::AckStatus::Ok,
            applied_at: std::time::SystemTime::now(),
            latency_ms: Some(5),
            error_code: None,
            error_message: None,
        };
        let text = serde_json::to_string(&AckEnvelope {
            kind: "ack".into(),
            payload: base_ack.clone(),
        })
        .unwrap();
        let ack = parse_action_ack(&text).expect("parsed ack");
        assert_eq!(ack.id, "a1");
    }

    #[test]
    fn parses_state_envelope() {
        let base_diff = StateDiff {
            sequence: 7,
            emitted_at: std::time::SystemTime::now(),
            payload: serde_json::json!({"ops": []}),
        };
        let text = serde_json::to_string(&StateEnvelope {
            kind: "state".into(),
            payload: base_diff.clone(),
        })
        .unwrap();
        let diff = parse_state_diff(&text).expect("parsed state");
        assert_eq!(diff.sequence, 7);
    }

    #[test]
    fn action_bytes_extracts_payload() {
        let action = ActionCommand {
            id: "a1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({ "bytes": "hello" }),
            expires_at: None,
        };
        let bytes = fast_path_action_bytes(&action).expect("bytes extracted");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn action_bytes_rejects_non_terminal() {
        let action = ActionCommand {
            id: "a1".into(),
            action_type: "resize".into(),
            payload: serde_json::json!({ "rows": 10, "cols": 20 }),
            expires_at: None,
        };
        assert!(fast_path_action_bytes(&action).is_err());
    }

    #[test]
    fn decode_chunked_text_reassembles_payload() {
        let runtime = Runtime::new().expect("runtime");
        runtime.block_on(async {
            let payload = "A".repeat(64 * 1024);
            let frames =
                frame_fast_path_payload(FastPathPayloadKind::Acks, &payload).expect("chunk");
            assert!(frames.len() > 1);
            let reassembler = Arc::new(Mutex::new(FastPathChunkReassembler::new(
                FastPathPayloadKind::Acks,
            )));
            let mut decoded = None;
            for frame in frames {
                let msg = DataChannelMessage {
                    is_string: true,
                    data: Bytes::from(frame),
                };
                if let Some(text) = decode_chunked_text(&reassembler, &msg)
                    .await
                    .expect("decode chunk")
                {
                    decoded = Some(text);
                }
            }
            assert_eq!(decoded.expect("assembled payload"), payload);
        });
    }
}

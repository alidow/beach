use crate::auth::error::AuthError;
use crate::auth::{self, FRIENDLY_FALLBACK_MESSAGE};
use crate::protocol::{self, HostFrame};
use crate::session::{SessionHandle, SessionRole, TransportOffer};
use crate::terminal::error::CliError;
use crate::transport as transport_mod;
use crate::transport::{Transport, TransportError, TransportId, TransportKind, TransportMessage};
use beach_lifeguard_client::{ClientHello, ServerHello};
use beach_lifeguard_core::TelemetryPreference;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};
use transport_mod::webrtc::{
    OffererSupervisor, SignalingClient, WebRtcChannels, WebRtcConnection, WebRtcRole,
};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct NegotiatedSingle {
    pub transport: Arc<dyn Transport>,
    pub webrtc_channels: Option<WebRtcChannels>,
    pub signaling_client: Option<Arc<SignalingClient>>,
    pub metadata: HashMap<String, String>,
}

pub enum NegotiatedTransport {
    Single(NegotiatedSingle),
    WebRtcOfferer {
        supervisor: Arc<OffererSupervisor>,
        connection: WebRtcConnection,
        peer_id: String,
        handshake_id: String,
        metadata: HashMap<String, String>,
    },
}

pub async fn negotiate_transport(
    handle: &SessionHandle,
    passphrase: Option<&str>,
    client_label: Option<&str>,
    request_mcp_channel: bool,
    metadata: Option<HashMap<String, String>>,
) -> Result<NegotiatedTransport, CliError> {
    let mut errors = Vec::new();
    let mut metadata = metadata.unwrap_or_default();
    metadata
        .entry("host_session_id".to_string())
        .or_insert_with(|| handle.session_id().to_string());

    let offers: Vec<TransportOffer> = handle.offers().to_vec();

    const HOST_ROLE_CANDIDATES: [WebRtcRole; 2] = [WebRtcRole::Offerer, WebRtcRole::Answerer];
    const PARTICIPANT_ROLE_CANDIDATES: [WebRtcRole; 1] = [WebRtcRole::Answerer];

    let role_candidates: &[WebRtcRole] = match handle.role() {
        SessionRole::Host => &HOST_ROLE_CANDIDATES,
        SessionRole::Participant => &PARTICIPANT_ROLE_CANDIDATES,
    };

    for &preferred_role in role_candidates {
        for offer in &offers {
            let TransportOffer::WebRtc { offer } = offer else {
                continue;
            };
            let offer_json =
                serde_json::to_string(&offer).unwrap_or_else(|_| "<invalid offer>".into());
            trace!(target = "session::webrtc", preferred = ?preferred_role, offer = %offer_json);
            let Some(signaling_url_str) = offer.get("signaling_url").and_then(Value::as_str) else {
                errors.push("webrtc offer missing signaling_url".to_string());
                continue;
            };

            // Rewrite signaling URL to match the internal Docker URL if configured.
            // This ensures that if we are running in the container (Host), we use the internal URL (e.g. http://beach-road:4132)
            // instead of the public URL (e.g. http://127.0.0.1:4132) which is unreachable from inside the container.
            // We use the BEACH_ROAD_URL environment variable which is injected by pong-stack.sh.
            let signaling_url = if let Ok(mut url) = Url::parse(signaling_url_str) {
                let rewritten = if let Ok(internal_url_str) = env::var("BEACH_ROAD_URL") {
                    if let Ok(internal_url) = Url::parse(internal_url_str.trim()) {
                        let _ = url.set_scheme(internal_url.scheme());
                        if let Some(internal_host) = internal_url.host_str() {
                            let _ = url.set_host(Some(internal_host));
                        }
                        let _ = url.set_port(internal_url.port());
                        Some(url.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };
                rewritten.unwrap_or_else(|| signaling_url_str.to_string())
            } else {
                signaling_url_str.to_string()
            };
            let meta_role = match offer.get("role").and_then(Value::as_str) {
                Some("offerer") => WebRtcRole::Offerer,
                Some("answerer") | None => WebRtcRole::Answerer,
                Some(other) => {
                    errors.push(format!("unsupported webrtc role {other}"));
                    continue;
                }
            };

            // Force host to attempt Offerer first even if metadata says Answerer.
            // This avoids deadlocks when both ends are incorrectly assigned Answerer.
            let effective_role = if matches!(handle.role(), SessionRole::Host)
                && matches!(preferred_role, WebRtcRole::Offerer)
            {
                if !matches!(meta_role, WebRtcRole::Offerer) {
                    warn!(
                        transport = "webrtc",
                        signaling_url = %signaling_url,
                        metadata_role = ?meta_role,
                        "overriding metadata: host will act as offerer"
                    );
                }
                WebRtcRole::Offerer
            } else {
                meta_role
            };

            let role_matches = matches!(preferred_role, WebRtcRole::Offerer)
                && matches!(effective_role, WebRtcRole::Offerer)
                || matches!(preferred_role, WebRtcRole::Answerer)
                    && matches!(effective_role, WebRtcRole::Answerer);
            if !role_matches {
                continue;
            }
            let poll_ms = offer
                .get("poll_interval_ms")
                .and_then(Value::as_u64)
                .unwrap_or(250);

            debug!(transport = "webrtc", signaling_url = %signaling_url, role = ?effective_role, "attempting webrtc transport");
            match effective_role {
                WebRtcRole::Offerer => match OffererSupervisor::connect(
                    &signaling_url,
                    Duration::from_millis(poll_ms),
                    passphrase,
                    request_mcp_channel,
                    Some(metadata.clone()),
                )
                .await
                {
                    Ok((supervisor, accepted)) => {
                        let metadata = accepted.metadata.clone();
                        info!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            peer_id = %accepted.peer_id,
                            handshake_id = %accepted.handshake_id,
                            "transport established"
                        );
                        return Ok(NegotiatedTransport::WebRtcOfferer {
                            supervisor,
                            connection: accepted.connection,
                            peer_id: accepted.peer_id,
                            handshake_id: accepted.handshake_id,
                            metadata,
                        });
                    }
                    Err(err) => {
                        warn!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            role = ?effective_role,
                            error = %err,
                            "webrtc negotiation failed"
                        );
                        errors.push(format!("webrtc {signaling_url}: {err}"));
                    }
                },
                WebRtcRole::Answerer => match transport_mod::webrtc::connect_via_signaling(
                    &signaling_url,
                    effective_role,
                    Duration::from_millis(poll_ms),
                    passphrase,
                    client_label,
                    request_mcp_channel,
                    Some(metadata.clone()),
                )
                .await
                {
                    Ok(connection) => {
                        info!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            role = ?effective_role,
                            "transport established"
                        );
                        return Ok(NegotiatedTransport::Single(NegotiatedSingle {
                            transport: connection.transport(),
                            webrtc_channels: Some(connection.channels()),
                            signaling_client: connection.signaling_client(),
                            metadata: connection.metadata().unwrap_or_default(),
                        }));
                    }
                    Err(err) => {
                        warn!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            role = ?effective_role,
                            error = %err,
                            "webrtc negotiation failed"
                        );
                        errors.push(format!("webrtc {signaling_url}: {err}"));
                    }
                },
            }
        }
    }

    for offer in &offers {
        if let TransportOffer::WebSocket { url } = offer {
            let url = if let Ok(mut parsed) = Url::parse(url) {
                let rewritten = if let Ok(internal_url_str) = env::var("BEACH_ROAD_URL") {
                    if let Ok(internal_url) = Url::parse(internal_url_str.trim()) {
                        if let Some(internal_host) = internal_url.host_str() {
                            let _ = parsed.set_host(Some(internal_host));
                        }
                        let _ = parsed.set_port(internal_url.port());

                        // Fix scheme for WebSocket
                        match internal_url.scheme() {
                            "http" => {
                                let _ = parsed.set_scheme("ws");
                            }
                            "https" => {
                                let _ = parsed.set_scheme("wss");
                            }
                            s => {
                                let _ = parsed.set_scheme(s);
                            }
                        }

                        Some(parsed.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };
                rewritten.unwrap_or_else(|| url.clone())
            } else {
                url.clone()
            };
            debug!(transport = "websocket", url = %url, "attempting websocket transport");
            match transport_mod::websocket::connect(&url).await {
                Ok(transport) => {
                    let transport = Arc::from(transport);
                    info!(transport = "websocket", url = %url, "transport established");
                    return Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport,
                        webrtc_channels: None,
                        signaling_client: None,
                        metadata: HashMap::new(),
                    }));
                }
                Err(err) => {
                    warn!(transport = "websocket", url = %url, error = %err, "websocket negotiation failed");
                    errors.push(format!("websocket {url}: {err}"));
                }
            }
        }
    }

    for offer in &offers {
        if let TransportOffer::WebSocketFallback { url } = offer {
            debug!(transport = "websocket_fallback", url = %url, "attempting ws fallback transport");
            match connect_fallback_websocket(handle, url).await {
                Ok(transport) => {
                    info!(transport = "websocket_fallback", url = %url, "transport established");
                    return Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport,
                        webrtc_channels: None,
                        signaling_client: None,
                        metadata: HashMap::new(),
                    }));
                }
                Err(err) => {
                    warn!(transport = "websocket_fallback", url = %url, error = %err, "websocket fallback negotiation failed");
                    errors.push(format!("websocket_fallback {url}: {err}"));
                }
            }
        }
    }

    if errors.is_empty() {
        Err(CliError::NoUsableTransport)
    } else {
        Err(CliError::TransportNegotiation(errors.join("; ")))
    }
}

async fn connect_fallback_websocket(
    handle: &SessionHandle,
    websocket_url: &str,
) -> Result<Arc<dyn Transport>, String> {
    #[derive(Serialize)]
    struct FallbackTokenRequest {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cohort_id: Option<String>,
        #[serde(default)]
        telemetry_opt_in: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        total_sessions_hint: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entitlement_proof: Option<String>,
    }

    #[derive(Deserialize)]
    struct FallbackTokenResponse {
        token: String,
        #[allow(dead_code)]
        expires_at: OffsetDateTime,
        guardrail_ratio: f64,
        guardrail_soft_breach: bool,
        telemetry_enabled: bool,
    }

    let token_url = handle
        .session_url()
        .join("../fallback/token")
        .map_err(|err| format!("invalid fallback token url: {err}"))?;

    let session_uuid =
        Uuid::parse_str(handle.session_id()).map_err(|err| format!("invalid session id: {err}"))?;

    let client = Client::new();
    let cohort_override = fallback_cohort_override();
    let telemetry_opt_in = fallback_telemetry_opt_in();
    let entitlement = match fallback_entitlement_override() {
        Some(value) => Some(value),
        None => match auth::resolve_fallback_access_token(None).await {
            Ok(token) => Some(token),
            Err(err) => {
                return Err(match err {
                    AuthError::NotLoggedIn
                    | AuthError::ProfileNotFound(_)
                    | AuthError::FallbackNotEntitled => {
                        FRIENDLY_FALLBACK_MESSAGE.to_string()
                    }
                    AuthError::PassphraseMissing => "Beach Auth credentials are locked. Set BEACH_AUTH_PASSPHRASE to unlock them before retrying WebSocket fallback.".to_string(),
                    other => {
                        warn!(
                            target: "beach::fallback",
                            error = %other,
                            "failed to resolve Beach Auth entitlement proof"
                        );
                        format!("failed to resolve Beach Auth entitlement proof: {other}")
                    }
                });
            }
        },
    };
    let request = FallbackTokenRequest {
        session_id: handle.session_id().to_string(),
        cohort_id: cohort_override,
        telemetry_opt_in,
        total_sessions_hint: None,
        entitlement_proof: entitlement,
    };

    let response = client
        .post(token_url.clone())
        .json(&request)
        .send()
        .await
        .map_err(|err| format!("token request failed: {err}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("failed to read token response body: {err}"))?;

    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<FallbackTokenErrorBody>(&body) {
            if let Some(reason) = error.reason.as_deref() {
                return Err(match reason {
                    "fallback_paused" => {
                        "websocket fallback disabled by operator (kill switch engaged)".to_string()
                    }
                    "entitlement_required" => FRIENDLY_FALLBACK_MESSAGE.to_string(),
                    "invalid_session_id" => "server rejected session identifier".to_string(),
                    other => {
                        format!("token request failed ({status}): server reported reason '{other}'")
                    }
                });
            }
        }
        return Err(format!(
            "token request failed ({status}): {body}",
            status = status
        ));
    }

    let token_response: FallbackTokenResponse = serde_json::from_str(&body)
        .map_err(|err| format!("failed to decode token response: {err} ({body})"))?;

    if token_response.guardrail_soft_breach {
        warn!(
            transport = "websocket_fallback",
            ratio = %token_response.guardrail_ratio,
            "fallback guardrail soft breach"
        );
    }

    let mut ws_url = Url::parse(websocket_url)
        .map_err(|err| format!("invalid websocket url '{websocket_url}': {err}"))?;
    ws_url
        .query_pairs_mut()
        .append_pair("token", &token_response.token);

    let (mut stream, _response) = connect_async(ws_url.as_str())
        .await
        .map_err(|err| format!("websocket connect failed: {err}"))?;

    let telemetry_pref = if token_response.telemetry_enabled {
        TelemetryPreference::Enabled
    } else {
        TelemetryPreference::Disabled
    };

    let client_hello = ClientHello::new(session_uuid).with_telemetry(telemetry_pref);
    let payload = serde_json::to_string(&client_hello)
        .map_err(|err| format!("failed to encode client hello: {err}"))?;

    stream
        .send(Message::Text(payload))
        .await
        .map_err(|err| format!("failed to send client hello: {err}"))?;

    let server_msg = stream
        .next()
        .await
        .ok_or_else(|| "server closed during handshake".to_string())
        .and_then(|msg| msg.map_err(|err| format!("error receiving server hello: {err}")))?;

    match server_msg {
        Message::Text(text) => {
            serde_json::from_str::<ServerHello>(&text)
                .map_err(|err| format!("failed to parse server hello: {err}"))?;
        }
        Message::Binary(bytes) => {
            serde_json::from_slice::<ServerHello>(&bytes)
                .map_err(|err| format!("failed to parse server hello: {err}"))?;
        }
        Message::Close(frame) => {
            let reason = frame.map(|f| f.reason.to_string());
            return Err(format!("server closed during handshake: {:?}", reason));
        }
        other => {
            return Err(format!("unexpected server hello frame: {other:?}"));
        }
    }

    let id = transport_mod::next_transport_id();
    let peer = TransportId(0);
    let transport =
        transport_mod::websocket::wrap_stream(TransportKind::WebSocket, id, peer, stream);

    Ok(Arc::from(transport))
}

#[derive(Debug, Deserialize)]
struct FallbackTokenErrorBody {
    #[serde(rename = "success")]
    _success: bool,
    #[serde(default)]
    reason: Option<String>,
}

fn fallback_cohort_override() -> Option<String> {
    env::var("BEACH_FALLBACK_COHORT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn fallback_entitlement_override() -> Option<String> {
    env::var("BEACH_ENTITLEMENT_PROOF")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn fallback_telemetry_opt_in() -> bool {
    env::var("BEACH_FALLBACK_TELEMETRY_OPT_IN")
        .map(|value| matches_ignore_ascii_true(&value))
        .unwrap_or(false)
}

fn matches_ignore_ascii_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(crate) struct SharedTransport {
    inner: RwLock<Arc<dyn Transport>>,
    metadata: RwLock<Option<HashMap<String, String>>>,
}

impl SharedTransport {
    pub(crate) fn new(
        initial: Arc<dyn Transport>,
        metadata: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            inner: RwLock::new(initial),
            metadata: RwLock::new(metadata),
        }
    }

    pub(crate) fn swap(&self, next: Arc<dyn Transport>, metadata: Option<HashMap<String, String>>) {
        let mut guard = self.inner.write().expect("shared transport poisoned");
        *guard = next;
        let mut meta_guard = self
            .metadata
            .write()
            .expect("shared transport metadata poisoned");
        *meta_guard = metadata;
    }

    pub(crate) fn current(&self) -> Arc<dyn Transport> {
        self.inner
            .read()
            .expect("shared transport poisoned")
            .clone()
    }

    pub(crate) fn metadata(&self) -> Option<HashMap<String, String>> {
        self.metadata
            .read()
            .expect("shared transport metadata poisoned")
            .clone()
    }
}

impl fmt::Debug for SharedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.current();
        f.debug_struct("SharedTransport")
            .field("transport_id", &current.id())
            .field("transport_kind", &current.kind())
            .field("metadata", &self.metadata())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportPair;

    #[test]
    fn shared_transport_tracks_metadata() {
        let pair = TransportPair::new(TransportKind::Ipc);
        let client: Arc<dyn Transport> = Arc::from(pair.client);
        let server: Arc<dyn Transport> = Arc::from(pair.server);

        let mut meta = HashMap::new();
        meta.insert("label".to_string(), "pb-controller".to_string());
        let shared = SharedTransport::new(client.clone(), Some(meta.clone()));
        assert_eq!(shared.metadata(), Some(meta.clone()));

        let mut new_meta = HashMap::new();
        new_meta.insert("label".to_string(), "pb-controller-2".to_string());
        shared.swap(server.clone(), Some(new_meta.clone()));
        assert_eq!(shared.metadata(), Some(new_meta));
    }
}

impl Transport for SharedTransport {
    fn kind(&self) -> TransportKind {
        self.current().kind()
    }

    fn id(&self) -> TransportId {
        self.current().id()
    }

    fn peer(&self) -> TransportId {
        self.current().peer()
    }

    fn send(&self, message: TransportMessage) -> Result<(), TransportError> {
        self.current().send(message)
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        self.current().send_text(text)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        self.current().send_bytes(bytes)
    }

    fn recv(&self, timeout: Duration) -> Result<TransportMessage, TransportError> {
        self.current().recv(timeout)
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        self.current().try_recv()
    }
}

#[derive(Clone)]
pub(crate) struct TransportSupervisor {
    shared: Arc<SharedTransport>,
    session_handle: SessionHandle,
    passphrase: Option<String>,
    reconnecting: Arc<AsyncMutex<bool>>,
}

impl TransportSupervisor {
    pub(crate) fn new(
        shared: Arc<SharedTransport>,
        session_handle: SessionHandle,
        passphrase: Option<String>,
    ) -> Self {
        Self {
            shared,
            session_handle,
            passphrase,
            reconnecting: Arc::new(AsyncMutex::new(false)),
        }
    }

    pub(crate) fn schedule_reconnect(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut guard = this.reconnecting.lock().await;
            if *guard {
                return;
            }
            *guard = true;
            drop(guard);

            const MAX_ATTEMPTS: usize = 5;
            let mut delay = Duration::from_millis(250);
            for attempt in 1..=MAX_ATTEMPTS {
                match negotiate_transport(
                    &this.session_handle,
                    this.passphrase.as_deref(),
                    None,
                    false,
                    None,
                )
                .await
                {
                    Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport: new_transport,
                        metadata: new_metadata,
                        ..
                    })) => {
                        let kind = new_transport.kind();
                        let id = new_transport.id().0;
                        this.shared.swap(new_transport, Some(new_metadata));
                        info!(
                            target = "transport_mod::failover",
                            ?kind,
                            transport_id = id,
                            attempt,
                            "transport failover completed"
                        );
                        break;
                    }
                    Ok(NegotiatedTransport::WebRtcOfferer { connection, .. }) => {
                        let transport = connection.transport();
                        let kind = transport.kind();
                        let id = transport.id().0;
                        this.shared.swap(transport, connection.metadata());
                        info!(
                            target = "transport_mod::failover",
                            ?kind,
                            transport_id = id,
                            attempt,
                            "transport failover completed (offerer)"
                        );
                        break;
                    }
                    Err(err) => {
                        warn!(
                            target = "transport_mod::failover",
                            attempt,
                            error = %err,
                            "transport failover attempt failed"
                        );
                        if attempt == MAX_ATTEMPTS {
                            error!(
                                target = "transport_mod::failover",
                                "exhausted transport failover attempts"
                            );
                            break;
                        }
                        sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(5));
                    }
                }
            }

            let mut guard = this.reconnecting.lock().await;
            *guard = false;
        });
    }
}

#[derive(Clone)]
pub(crate) struct HeartbeatPublisher {
    transport: Arc<dyn Transport>,
    supervisor: Option<Arc<TransportSupervisor>>,
}

impl HeartbeatPublisher {
    pub(crate) fn new(
        transport: Arc<dyn Transport>,
        supervisor: Option<Arc<TransportSupervisor>>,
    ) -> Self {
        Self {
            transport,
            supervisor,
        }
    }

    pub(crate) fn spawn(self, interval: Duration, limit: Option<usize>) {
        tokio::spawn(async move {
            let mut count: usize = 0;
            loop {
                if let Some(max) = limit {
                    if count >= max {
                        break;
                    }
                }

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                if let Err(err) = send_heartbeat_frame(&self.transport, count as u64, now as u64) {
                    debug!(
                        target = "transport_mod::heartbeat",
                        transport_id = self.transport.id().0,
                        transport = ?self.transport.kind(),
                        error = %err,
                        "heartbeat send failed; scheduling reconnect"
                    );
                    if let Some(supervisor) = &self.supervisor {
                        supervisor.schedule_reconnect();
                        sleep(interval).await;
                        continue;
                    } else {
                        debug!(
                            target = "transport_mod::heartbeat",
                            transport_id = self.transport.id().0,
                            transport = ?self.transport.kind(),
                            error = %err,
                            "heartbeat publisher stopping after failed send"
                        );
                        break;
                    }
                }

                count += 1;
                sleep(interval).await;
            }
        });
    }
}

fn send_heartbeat_frame(
    transport: &Arc<dyn Transport>,
    seq: u64,
    timestamp_ms: u64,
) -> Result<(), TransportError> {
    let frame = HostFrame::Heartbeat { seq, timestamp_ms };
    let bytes = protocol::encode_host_frame_binary(&frame);
    match transport.send_bytes(&bytes) {
        Ok(sequence) => {
            trace!(
                target = "transport_mod::heartbeat",
                transport_id = transport.id().0,
                transport = ?transport.kind(),
                sequence,
                "heartbeat frame sent"
            );
            Ok(())
        }
        Err(err) => {
            debug!(
                target = "transport_mod::heartbeat",
                transport_id = transport.id().0,
                transport = ?transport.kind(),
                error = %err,
                "failed to send heartbeat"
            );
            Err(err)
        }
    }
}

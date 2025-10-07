use crate::protocol::{self, HostFrame};
use crate::session::{SessionHandle, SessionRole, TransportOffer};
use crate::terminal::error::CliError;
use crate::transport as transport_mod;
use crate::transport::{Transport, TransportError, TransportId, TransportKind, TransportMessage};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use transport_mod::webrtc::{OffererSupervisor, WebRtcChannels, WebRtcConnection, WebRtcRole};

#[derive(Clone)]
pub struct NegotiatedSingle {
    pub transport: Arc<dyn Transport>,
    pub webrtc_channels: Option<WebRtcChannels>,
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
) -> Result<NegotiatedTransport, CliError> {
    let mut errors = Vec::new();

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
            let Some(signaling_url) = offer.get("signaling_url").and_then(Value::as_str) else {
                errors.push("webrtc offer missing signaling_url".to_string());
                continue;
            };
            let role = match offer.get("role").and_then(Value::as_str) {
                Some("offerer") => WebRtcRole::Offerer,
                Some("answerer") | None => WebRtcRole::Answerer,
                Some(other) => {
                    errors.push(format!("unsupported webrtc role {other}"));
                    continue;
                }
            };

            let role_matches = matches!(preferred_role, WebRtcRole::Offerer)
                && matches!(role, WebRtcRole::Offerer)
                || matches!(preferred_role, WebRtcRole::Answerer)
                    && matches!(role, WebRtcRole::Answerer);
            if !role_matches {
                continue;
            }
            let poll_ms = offer
                .get("poll_interval_ms")
                .and_then(Value::as_u64)
                .unwrap_or(250);

            debug!(transport = "webrtc", signaling_url = %signaling_url, ?role, "attempting webrtc transport");
            match role {
                WebRtcRole::Offerer => match OffererSupervisor::connect(
                    signaling_url,
                    Duration::from_millis(poll_ms),
                    passphrase,
                    request_mcp_channel,
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
                            ?role,
                            error = %err,
                            "webrtc negotiation failed"
                        );
                        errors.push(format!("webrtc {signaling_url}: {err}"));
                    }
                },
                WebRtcRole::Answerer => match transport_mod::webrtc::connect_via_signaling(
                    signaling_url,
                    role,
                    Duration::from_millis(poll_ms),
                    passphrase,
                    client_label,
                    request_mcp_channel,
                )
                .await
                {
                    Ok(connection) => {
                        info!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            ?role,
                            "transport established"
                        );
                        return Ok(NegotiatedTransport::Single(NegotiatedSingle {
                            transport: connection.transport(),
                            webrtc_channels: Some(connection.channels()),
                        }));
                    }
                    Err(err) => {
                        warn!(
                            transport = "webrtc",
                            signaling_url = %signaling_url,
                            ?role,
                            error = %err,
                            "webrtc negotiation failed"
                        );
                        errors.push(format!("webrtc {signaling_url}: {err}"));
                    }
                },
            }
        }
    }

    for offer in offers {
        if let TransportOffer::WebSocket { url } = offer {
            debug!(transport = "websocket", url = %url, "attempting websocket transport");
            match transport_mod::websocket::connect(&url).await {
                Ok(transport) => {
                    let transport = Arc::from(transport);
                    info!(transport = "websocket", url = %url, "transport established");
                    return Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport,
                        webrtc_channels: None,
                    }));
                }
                Err(err) => {
                    warn!(transport = "websocket", url = %url, error = %err, "websocket negotiation failed");
                    errors.push(format!("websocket {url}: {err}"));
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

pub(crate) struct SharedTransport {
    inner: RwLock<Arc<dyn Transport>>,
}

impl SharedTransport {
    pub(crate) fn new(initial: Arc<dyn Transport>) -> Self {
        Self {
            inner: RwLock::new(initial),
        }
    }

    pub(crate) fn swap(&self, next: Arc<dyn Transport>) {
        let mut guard = self.inner.write().expect("shared transport poisoned");
        *guard = next;
    }

    pub(crate) fn current(&self) -> Arc<dyn Transport> {
        self.inner
            .read()
            .expect("shared transport poisoned")
            .clone()
    }
}

impl fmt::Debug for SharedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.current();
        f.debug_struct("SharedTransport")
            .field("transport_id", &current.id())
            .field("transport_kind", &current.kind())
            .finish()
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
                )
                .await
                {
                    Ok(NegotiatedTransport::Single(NegotiatedSingle {
                        transport: new_transport,
                        ..
                    })) => {
                        let kind = new_transport.kind();
                        let id = new_transport.id().0;
                        this.shared.swap(new_transport);
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
                        this.shared.swap(transport);
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

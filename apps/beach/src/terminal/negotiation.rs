use crate::session::{SessionHandle, SessionRole, TransportOffer};
use crate::terminal::error::CliError;
use crate::transport as transport_mod;
use crate::transport::Transport;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};
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

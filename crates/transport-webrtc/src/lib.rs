use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use beach_client_core::protocol::{
    self, ClientFrame, ExtensionFrame as BeachExtensionFrame, HostFrame,
};
use beach_client_core::transport::webrtc::{WebRtcRole, connect_via_signaling};
use beach_client_core::transport::{self, ExtensionDirection, ExtensionLane, Payload, Transport};
use bytes::Bytes;
use parking_lot::RwLock;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use transport_bus::{Bus, BusError};
use transport_unified_adapter::{
    ExtensionFrame, ExtensionTransport, UnifiedBus, UnifiedBusAdapter, UnifiedBusError,
};
use url::Url;

const DEFAULT_NAMESPACE: &str = "manager";
const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const MANAGER_LABEL: &str = "beach-manager";

fn sender_for(
    namespaces: &Arc<RwLock<HashMap<String, broadcast::Sender<ExtensionFrame>>>>,
    namespace: &str,
) -> broadcast::Sender<ExtensionFrame> {
    let mut guard = namespaces.write();
    guard
        .entry(namespace.to_string())
        .or_insert_with(|| broadcast::channel(256).0)
        .clone()
}

fn broadcast_frame(
    namespaces: &Arc<RwLock<HashMap<String, broadcast::Sender<ExtensionFrame>>>>,
    frame: ExtensionFrame,
) {
    let sender = sender_for(namespaces, &frame.namespace);
    let _ = sender.send(frame);
}

struct WebRtcExtensionTransport {
    transport: Arc<dyn Transport>,
    direction: ExtensionDirection,
    namespaces: Arc<RwLock<HashMap<String, broadcast::Sender<ExtensionFrame>>>>,
    _pump: tokio::task::JoinHandle<()>,
}

impl WebRtcExtensionTransport {
    fn new(transport: Arc<dyn Transport>, direction: ExtensionDirection) -> Self {
        let namespaces = Arc::new(RwLock::new(HashMap::new()));
        let pump = Self::spawn_extension_pump(transport.clone(), namespaces.clone());
        Self {
            transport,
            direction,
            namespaces,
            _pump: pump,
        }
    }

    fn spawn_extension_pump(
        transport: Arc<dyn Transport>,
        namespaces: Arc<RwLock<HashMap<String, broadcast::Sender<ExtensionFrame>>>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            loop {
                match transport.recv(Duration::from_millis(250)) {
                    Ok(message) => {
                        let Payload::Binary(bytes) = message.payload else {
                            continue;
                        };
                        if let Ok(frame) = protocol::decode_host_frame_binary(&bytes) {
                            if let HostFrame::Extension { frame } = frame {
                                let outbound = ExtensionFrame {
                                    namespace: frame.namespace.clone(),
                                    topic: frame.kind.clone(),
                                    payload: frame.payload.clone(),
                                };
                                broadcast_frame(&namespaces, outbound);
                                transport::extensions::publish(transport.id(), frame);
                            }
                            continue;
                        }
                        if let Ok(frame) = protocol::decode_client_frame_binary(&bytes) {
                            if let ClientFrame::Extension { frame } = frame {
                                let outbound = ExtensionFrame {
                                    namespace: frame.namespace.clone(),
                                    topic: frame.kind.clone(),
                                    payload: frame.payload.clone(),
                                };
                                broadcast_frame(&namespaces, outbound);
                                transport::extensions::publish(transport.id(), frame);
                            }
                        }
                    }
                    Err(transport::TransportError::Timeout) => continue,
                    Err(transport::TransportError::ChannelClosed) => break,
                    Err(err) => {
                        warn!(error = %err, "rtc extension pump stopping");
                        break;
                    }
                }
            }
        })
    }
}

impl ExtensionTransport for WebRtcExtensionTransport {
    fn subscribe_extensions(&self, namespace: &str) -> broadcast::Receiver<ExtensionFrame> {
        sender_for(&self.namespaces, namespace).subscribe()
    }

    fn send_extension(&self, namespace: &str, topic: &str, payload: Bytes) -> Result<(), BusError> {
        let frame = BeachExtensionFrame {
            namespace: namespace.to_string(),
            kind: topic.to_string(),
            payload: payload.clone(),
        };
        self.transport
            .send_extension(self.direction, frame.clone(), ExtensionLane::ControlOrdered)
            .map_err(|err| BusError::Transport(err.to_string()))?;
        let outbound = ExtensionFrame {
            namespace: namespace.to_string(),
            topic: topic.to_string(),
            payload,
        };
        broadcast_frame(&self.namespaces, outbound);
        transport::extensions::publish(self.transport.id(), frame);
        Ok(())
    }

    fn id(&self) -> String {
        format!("{:?}", self.transport.id())
    }
}

fn session_base_from_env(base: &str) -> String {
    std::env::var("BEACH_ROAD_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| base.to_string())
}

fn build_signaling_url(base: &str, host_session_id: &str) -> Result<String, WebRtcError> {
    let mut url = Url::parse(base)
        .map_err(|err| WebRtcError::Setup(format!("invalid session base {base}: {err}")))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| WebRtcError::Setup("cannot mutate signaling url path".into()))?;
        segments.push("sessions");
        segments.push(host_session_id);
        segments.push("webrtc");
    }
    Ok(url.to_string())
}

fn poll_interval_ms() -> u64 {
    std::env::var("BEACH_WEBRTC_POLL_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
}

/// Build a negotiated ExtensionTransport backed by the unified WebRTC channel.
pub async fn attach_and_build_transport(
    session_server_base: &str,
    host_session_id: &str,
) -> Result<Arc<dyn ExtensionTransport>, WebRtcError> {
    let session_base = session_base_from_env(session_server_base);
    let signaling_url = build_signaling_url(&session_base, host_session_id)?;
    let mut metadata = HashMap::new();
    metadata.insert("host_session_id".to_string(), host_session_id.to_string());
    if let Ok(instance) = std::env::var("BEACH_MANAGER_INSTANCE_ID") {
        metadata.insert("manager_instance_id".to_string(), instance);
    }
    info!(
        signaling_url,
        host_session_id, "attaching rtc transport to host session"
    );
    let connection = connect_via_signaling(
        signaling_url.as_str(),
        WebRtcRole::Answerer,
        Duration::from_millis(poll_interval_ms()),
        None,
        Some(MANAGER_LABEL),
        false,
        Some(metadata),
    )
    .await
    .map_err(WebRtcError::Transport)?;
    let transport =
        WebRtcExtensionTransport::new(connection.transport(), ExtensionDirection::ClientToHost);
    debug!(
        transport_id = %transport.id(),
        namespace = DEFAULT_NAMESPACE,
        "rtc transport ready"
    );
    Ok(Arc::new(transport))
}

/// Adapter that builds a UnifiedBus over an RTC transport for a given host session id.
pub struct RtcUnifiedAdapter {
    session_base: String,
    namespace: String,
}

impl RtcUnifiedAdapter {
    pub fn new(session_base: impl Into<String>) -> Self {
        Self {
            session_base: session_base.into(),
            namespace: DEFAULT_NAMESPACE.to_string(),
        }
    }

    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }
}

#[async_trait]
impl UnifiedBusAdapter for RtcUnifiedAdapter {
    async fn build_bus(&self, host_session_id: &str) -> Result<Arc<dyn Bus>, UnifiedBusError> {
        let transport = attach_and_build_transport(&self.session_base, host_session_id)
            .await
            .map_err(|err| UnifiedBusError::NotReady(err.to_string()))?;
        Ok(Arc::new(UnifiedBus::new(transport, self.namespace.clone())))
    }
}

#[derive(Debug, Error)]
pub enum WebRtcError {
    #[error("transport setup failed: {0}")]
    Setup(String),
    #[error("webrtc negotiation failed: {0}")]
    Transport(#[from] transport::TransportError),
}

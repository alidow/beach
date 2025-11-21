use std::sync::Arc;

use async_trait::async_trait;
use beach_buggy::{
    ActionAck, ActionCommand, ControllerPairing, ControllerPairingStream, ControllerTransport,
    HarnessError, HarnessResult, HealthHeartbeat, ManagerTransport, RegisterSessionRequest,
    RegisterSessionResponse, StateDiff, fast_path::parse_action_payload,
};
use futures::stream;
use serde_json::json;
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, info, trace, warn};

use crate::metrics;
use crate::protocol::{ExtensionFrame, HostFrame, decode_host_frame_binary};
use crate::transport::{ExtensionDirection, ExtensionLane, Transport, extensions};

const FASTPATH_NAMESPACE: &str = "fastpath";
const KIND_ACTION: &str = "action";
const KIND_ACK: &str = "ack";
const KIND_STATE: &str = "state";
const KIND_HEALTH: &str = "health";

/// Bridge that adapts a negotiated [`Transport`] to Beach Buggy's `ManagerTransport` trait
/// using unified transport extension frames.
#[derive(Clone)]
pub struct UnifiedBuggyTransport {
    transport: Arc<dyn Transport>,
    actions_tx: broadcast::Sender<ActionCommand>,
    actions_rx: Arc<Mutex<broadcast::Receiver<ActionCommand>>>,
    use_transport_pump: bool,
}

impl UnifiedBuggyTransport {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::new_with_pump(transport, true)
    }

    /// Creates a bridge that relies on external extension publication (via the
    /// transport's namespace bus) instead of draining the transport directly.
    /// Use this when the transport is already being consumed by another reader
    /// (e.g. the host input listener) to avoid racing on `recv`.
    pub fn new_with_subscription(transport: Arc<dyn Transport>) -> Self {
        Self::new_with_pump(transport, false)
    }

    fn new_with_pump(transport: Arc<dyn Transport>, use_transport_pump: bool) -> Self {
        let (actions_tx, actions_rx) = broadcast::channel(64);
        let transport_clone = transport.clone();
        let actions_tx_clone = actions_tx.clone();
        // Prime the channel with any buffered frames when we own the transport
        // reader; otherwise we rely on external subscribers to deliver frames.
        if use_transport_pump {
            pump_pending_extensions(&transport_clone, &actions_tx_clone);
        }
        Self {
            transport,
            actions_tx,
            actions_rx: Arc::new(Mutex::new(actions_rx)),
            use_transport_pump,
        }
    }

    fn make_frame(kind: &str, payload: impl Into<Vec<u8>>) -> ExtensionFrame {
        ExtensionFrame {
            namespace: FASTPATH_NAMESPACE.to_string(),
            kind: kind.to_string(),
            payload: payload.into().into(),
        }
    }

    async fn next_actions(&self) -> HarnessResult<Vec<ActionCommand>> {
        if self.use_transport_pump {
            pump_pending_extensions(&self.transport, &self.actions_tx);
        }
        let mut rx = self.actions_rx.lock().await;
        let mut actions = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(cmd) => actions.push(cmd),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    return Err(HarnessError::Transport("actions stream closed".into()));
                }
                Err(broadcast::error::TryRecvError::Lagged(skipped)) => {
                    warn!(
                        target = "unified_transport",
                        skipped, "lagged receiving actions"
                    );
                    continue;
                }
            }
        }
        Ok(actions)
    }

    async fn send_extension_frame(
        &self,
        direction: ExtensionDirection,
        frame: ExtensionFrame,
    ) -> HarnessResult<()> {
        self.transport
            .send_extension(direction, frame.clone(), ExtensionLane::ControlOrdered)
            .map(|_| ())
            .map_err(|err| HarnessError::Transport(err.to_string()))
            .map(|_| {
                metrics::EXTENSION_SENT
                    .with_label_values(&[
                        &frame.namespace,
                        &frame.kind,
                        direction_role(direction),
                        "unified",
                    ])
                    .inc();
            })
    }

    fn encode_payload(kind: &str, value: serde_json::Value) -> HarnessResult<Vec<u8>> {
        let envelope = json!({ "type": kind, "payload": value });
        serde_json::to_vec(&envelope)
            .map_err(|err| HarnessError::Transport(format!("encode payload failed: {err}")))
    }

    pub fn ingest_extension_frame(&self, frame: ExtensionFrame) {
        handle_extension_frame(&self.transport, &self.actions_tx, frame);
    }
}

fn direction_role(direction: ExtensionDirection) -> &'static str {
    match direction {
        ExtensionDirection::HostToClient => "host",
        ExtensionDirection::ClientToHost => "controller",
    }
}

#[async_trait]
impl ManagerTransport for UnifiedBuggyTransport {
    async fn register_session(
        &self,
        request: RegisterSessionRequest,
    ) -> HarnessResult<RegisterSessionResponse> {
        // In unified transport mode the harness attaches to an already-negotiated transport.
        // Registration still happens locally with a synthetic response so downstream flows can start.
        info!(
            target = "unified_transport",
            session_id = %request.session_id,
            "register_session handled locally (unified transport mode)"
        );
        Ok(RegisterSessionResponse {
            harness_id: format!("unified-{}", request.session_id),
            controller_token: None,
            lease_ttl_ms: 30_000,
            state_cache_url: None,
            transport_hints: Default::default(),
        })
    }

    async fn send_state(&self, _session_id: &str, diff: StateDiff) -> HarnessResult<()> {
        let payload = Self::encode_payload(KIND_STATE, json!(diff))?;
        self.send_extension_frame(
            ExtensionDirection::ClientToHost,
            Self::make_frame(KIND_STATE, payload),
        )
        .await
    }

    async fn receive_actions(&self, _session_id: &str) -> HarnessResult<Vec<ActionCommand>> {
        self.next_actions().await
    }

    async fn ack_actions(&self, _session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()> {
        for ack in acks {
            let payload = Self::encode_payload(KIND_ACK, json!(ack))?;
            self.send_extension_frame(
                ExtensionDirection::ClientToHost,
                Self::make_frame(KIND_ACK, payload),
            )
            .await?;
        }
        Ok(())
    }

    async fn signal_health(
        &self,
        _session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> HarnessResult<()> {
        let payload = Self::encode_payload(KIND_HEALTH, json!(heartbeat))?;
        self.send_extension_frame(
            ExtensionDirection::ClientToHost,
            Self::make_frame(KIND_HEALTH, payload),
        )
        .await
    }
}

#[async_trait]
impl ControllerTransport for UnifiedBuggyTransport {
    async fn list_controller_pairings(
        &self,
        _controller_session_id: &str,
    ) -> HarnessResult<Vec<ControllerPairing>> {
        Ok(Vec::new())
    }

    async fn stream_controller_pairings(
        &self,
        _controller_session_id: &str,
    ) -> HarnessResult<ControllerPairingStream> {
        Ok(Box::pin(stream::empty()))
    }

    async fn renew_controller_lease(
        &self,
        _session_id: &str,
        _ttl_ms: Option<u64>,
    ) -> HarnessResult<beach_buggy::ControllerLeaseRenewal> {
        Ok(beach_buggy::ControllerLeaseRenewal {
            controller_token: "unified-controller".into(),
            expires_at_ms: 30_000,
        })
    }
}

fn pump_pending_extensions(
    transport: &Arc<dyn Transport>,
    actions_tx: &broadcast::Sender<ActionCommand>,
) {
    loop {
        match transport.try_recv() {
            Ok(Some(message)) => {
                if let crate::transport::Payload::Binary(bytes) = message.payload {
                    match decode_host_frame_binary(&bytes) {
                        Ok(HostFrame::Extension { frame }) => {
                            handle_extension_frame(transport, actions_tx, frame);
                        }
                        Ok(_) => {}
                        Err(err) => debug!(
                            target = "unified_transport",
                            error = %err,
                            "failed to decode host frame in extension pump"
                        ),
                    }
                }
            }
            Ok(None) => break,
            Err(crate::transport::TransportError::Timeout) => {}
            Err(crate::transport::TransportError::ChannelClosed) => break,
            Err(crate::transport::TransportError::Setup(e)) => {
                warn!(target = "unified_transport", error = %e, "transport setup error");
                break;
            }
        }
    }
}

fn handle_extension_frame(
    transport: &Arc<dyn Transport>,
    actions_tx: &broadcast::Sender<ActionCommand>,
    frame: ExtensionFrame,
) {
    metrics::EXTENSION_RECEIVED
        .with_label_values(&[&frame.namespace, &frame.kind, "host"])
        .inc();
    trace!(
        target = "unified_transport",
        transport_id = %transport.id().0,
        namespace = %frame.namespace,
        kind = %frame.kind,
        payload_len = frame.payload.len(),
        "received extension frame"
    );

    if frame.namespace == FASTPATH_NAMESPACE && frame.kind == KIND_ACTION {
        match std::str::from_utf8(&frame.payload) {
            Ok(text) => match parse_action_payload(text) {
                Ok(action) => {
                    let _ = actions_tx.send(action);
                }
                Err(err) => warn!(
                    target = "unified_transport",
                    error = %err,
                    "failed to parse action extension frame"
                ),
            },
            Err(err) => warn!(
                target = "unified_transport",
                error = %err,
                "invalid utf8 in action extension payload"
            ),
        }
    } else {
        trace!(
            target = "unified_transport",
            transport_id = %transport.id().0,
            namespace = %frame.namespace,
            kind = %frame.kind,
            payload_len = frame.payload.len(),
            "extension frame ignored (namespace not handled)"
        );
        extensions::publish(transport.id(), frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientFrame;
    use crate::transport::{TransportKind, TransportPair};
    use std::time::Duration;

    #[tokio::test]
    async fn actions_flow_over_extension_frames() {
        let pair = TransportPair::new(TransportKind::Ipc);
        let controller: Arc<dyn Transport> = Arc::from(pair.client);
        let server: Arc<dyn Transport> = Arc::from(pair.server);

        let bridge = UnifiedBuggyTransport::new(controller);

        let action_payload = json!({
            "type": "action",
            "payload": {
                "id": "a-1",
                "action_type": "terminal_write",
                "payload": {"bytes": "echo hi"},
                "expires_at": null
            }
        });
        let frame = HostFrame::Extension {
            frame: ExtensionFrame {
                namespace: FASTPATH_NAMESPACE.to_string(),
                kind: KIND_ACTION.to_string(),
                payload: serde_json::to_vec(&action_payload).unwrap().into(),
            },
        };
        let bytes = crate::protocol::encode_host_frame_binary(&frame);
        server.send_bytes(&bytes).unwrap();

        let actions = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let actions = bridge.receive_actions("sess").await.unwrap();
                if !actions.is_empty() {
                    break actions;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("timed out waiting for actions");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "a-1");
    }

    #[tokio::test]
    async fn emits_state_and_ack_extensions() {
        let pair = TransportPair::new(TransportKind::Ipc);
        let controller: Arc<dyn Transport> = Arc::from(pair.client);
        let server: Arc<dyn Transport> = Arc::from(pair.server);

        let bridge = UnifiedBuggyTransport::new(controller);

        // Send state
        bridge
            .send_state(
                "sess",
                StateDiff {
                    sequence: 1,
                    emitted_at: std::time::SystemTime::now(),
                    payload: json!({"hello": "world"}),
                },
            )
            .await
            .unwrap();

        // Send ack
        bridge
            .ack_actions(
                "sess",
                vec![ActionAck {
                    id: "ack-1".into(),
                    status: beach_buggy::AckStatus::Ok,
                    applied_at: std::time::SystemTime::now(),
                    latency_ms: Some(5),
                    error_code: None,
                    error_message: None,
                }],
            )
            .await
            .unwrap();

        // Collect frames from server side
        let mut seen = Vec::new();
        for _ in 0..2 {
            let msg = server.recv(Duration::from_millis(200)).unwrap();
            if let crate::transport::Payload::Binary(bytes) = msg.payload {
                seen.push(bytes);
            }
        }

        let mut kinds = Vec::new();
        for bytes in seen {
            match crate::protocol::decode_client_frame_binary(&bytes).unwrap() {
                ClientFrame::Extension { frame } => kinds.push(frame.kind),
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert!(kinds.contains(&KIND_STATE.to_string()));
        assert!(kinds.contains(&KIND_ACK.to_string()));
    }

    #[tokio::test]
    async fn ignores_unknown_extension_namespace() {
        let pair = TransportPair::new(TransportKind::Ipc);
        let controller: Arc<dyn Transport> = Arc::from(pair.client);
        let server: Arc<dyn Transport> = Arc::from(pair.server);

        let bridge = UnifiedBuggyTransport::new(controller);

        let frame = HostFrame::Extension {
            frame: ExtensionFrame {
                namespace: "other".to_string(),
                kind: "noop".to_string(),
                payload: b"{}".to_vec().into(),
            },
        };
        let bytes = crate::protocol::encode_host_frame_binary(&frame);
        server.send_bytes(&bytes).unwrap();

        let actions = bridge.receive_actions("sess").await.unwrap();
        assert!(actions.is_empty());
    }
}

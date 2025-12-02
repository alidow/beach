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
use tracing::{info, warn};
use transport_bus::Bus;

use crate::protocol::{ExtensionFrame, HostFrame};
use crate::transport::{Transport, bus::manager_bus_from_client, extensions};
use transport_unified_adapter::UnifiedBus;

#[allow(dead_code)]
const BUS_NAMESPACE: &str = "manager";
#[allow(dead_code)]
const LEGACY_NAMESPACE: &str = "fastpath";
// Preferred topics
const TOPIC_ACTION: &str = "beach.manager.action";
const TOPIC_ACK: &str = "beach.manager.ack";
const TOPIC_STATE: &str = "beach.manager.state";
const TOPIC_HEALTH: &str = "beach.manager.health";
// Legacy topics still honored for compatibility (input only)
const LEGACY_TOPIC_ACTION: &str = "controller/input";
const KIND_ACTION: &str = "action";
const KIND_ACK: &str = "ack";
const KIND_STATE: &str = "state";
const KIND_HEALTH: &str = "health";

/// Bridge that adapts a negotiated [`Transport`] to Beach Buggy's `ManagerTransport` trait
/// using unified transport extension frames.
pub struct UnifiedBuggyTransport {
    transport: Arc<dyn Transport>,
    bus: Arc<UnifiedBus>,
    actions_tx: broadcast::Sender<ActionCommand>,
    actions_rx: Arc<Mutex<broadcast::Receiver<ActionCommand>>>,
}

impl UnifiedBuggyTransport {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::new_with_pump(transport)
    }

    /// Creates a bridge that relies on external extension publication (via the
    /// transport's namespace bus) instead of draining the transport directly.
    /// Use this when the transport is already being consumed by another reader
    /// (e.g. the host input listener) to avoid racing on `recv`.
    pub fn new_with_subscription(transport: Arc<dyn Transport>) -> Self {
        Self::new_with_pump(transport)
    }

    fn new_with_pump(transport: Arc<dyn Transport>) -> Self {
        let (actions_tx, actions_rx) = broadcast::channel(64);
        let bus = Arc::new(manager_bus_from_client(transport.clone()));
        // Pump raw extension frames from the transport into the bus so subscribers see them.
        {
            let transport_clone = transport.clone();
            let bus_clone = bus.clone();
            tokio::spawn(async move {
                loop {
                    match transport_clone.try_recv() {
                        Ok(Some(message)) => {
                            if let crate::transport::Payload::Binary(bytes) = message.payload {
                                if let Ok(HostFrame::Extension { frame }) =
                                    crate::protocol::decode_host_frame_binary(&bytes)
                                {
                                    let topic = frame.kind.clone();
                                    let _ =
                                        bus_clone.publish(topic.as_str(), frame.payload.clone());
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(crate::transport::TransportError::Timeout) => break,
                        Err(crate::transport::TransportError::ChannelClosed) => break,
                        Err(crate::transport::TransportError::Setup(_)) => break,
                    }
                }
            });
        }
        // Subscribe to bus topic for controller input and forward into actions_tx.
        {
            let mut bus_rx = bus.subscribe(TOPIC_ACTION);
            let actions_tx_forward = actions_tx.clone();
            tokio::spawn(async move {
                while let Ok(msg) = bus_rx.recv().await {
                    if let Ok(text) = std::str::from_utf8(&msg.payload) {
                        if let Ok(action) = parse_action_payload(text) {
                            let _ = actions_tx_forward.send(action);
                        }
                    }
                }
            });
        }
        Self {
            transport,
            bus,
            actions_tx,
            actions_rx: Arc::new(Mutex::new(actions_rx)),
        }
    }

    async fn next_actions(&self) -> HarnessResult<Vec<ActionCommand>> {
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

    fn encode_payload(kind: &str, value: serde_json::Value) -> HarnessResult<Vec<u8>> {
        let envelope = json!({ "type": kind, "payload": value });
        serde_json::to_vec(&envelope)
            .map_err(|err| HarnessError::Transport(format!("encode payload failed: {err}")))
    }

    async fn send_controller_frame(&self, kind: &str, payload: Vec<u8>) -> HarnessResult<()> {
        let topic = match kind {
            KIND_ACTION => TOPIC_ACTION,
            KIND_ACK => TOPIC_ACK,
            KIND_STATE => TOPIC_STATE,
            KIND_HEALTH => TOPIC_HEALTH,
            other => other,
        };
        self.bus
            .publish(topic, payload.into())
            .map_err(|err| HarnessError::Transport(err.to_string()))
    }

    pub fn ingest_extension_frame(&self, frame: ExtensionFrame) {
        let kind = frame.kind.as_str();
        match kind {
            TOPIC_ACTION | LEGACY_TOPIC_ACTION => {
                if let Ok(text) = std::str::from_utf8(&frame.payload) {
                    if let Ok(action) = parse_action_payload(text) {
                        let _ = self.actions_tx.send(action);
                    }
                }
            }
            TOPIC_ACK | TOPIC_STATE | TOPIC_HEALTH => {
                let _ = self.bus.publish(kind, frame.payload.clone());
            }
            _ => {
                extensions::publish(self.transport.id(), frame);
            }
        }
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
        self.send_controller_frame(TOPIC_STATE, payload).await
    }

    async fn receive_actions(&self, _session_id: &str) -> HarnessResult<Vec<ActionCommand>> {
        self.next_actions().await
    }

    async fn ack_actions(&self, _session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()> {
        for ack in acks {
            let payload = Self::encode_payload(KIND_ACK, json!(ack))?;
            self.send_controller_frame(TOPIC_ACK, payload).await?;
        }
        Ok(())
    }

    async fn signal_health(
        &self,
        _session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> HarnessResult<()> {
        let payload = Self::encode_payload(KIND_HEALTH, json!(heartbeat))?;
        self.send_controller_frame(TOPIC_HEALTH, payload).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientFrame;
    use crate::transport::{ExtensionDirection, TransportKind, TransportPair, bus::UnifiedBus};
    use std::time::Duration;
    use tokio::time::timeout;

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
                namespace: BUS_NAMESPACE.to_string(),
                kind: TOPIC_ACTION.to_string(),
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
        assert!(kinds.contains(&TOPIC_STATE.to_string()));
        assert!(kinds.contains(&TOPIC_ACK.to_string()));
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

    #[tokio::test]
    async fn bus_action_ack_roundtrip() {
        use beach_buggy::fast_path::parse_action_payload;

        let pair = TransportPair::new(TransportKind::Ipc);
        let server: Arc<dyn Transport> = Arc::from(pair.server);
        let bus = Arc::new(UnifiedBus::new(
            server,
            ExtensionDirection::HostToClient,
            BUS_NAMESPACE,
        ));

        let mut action_rx = bus.subscribe(TOPIC_ACTION);
        let mut ack_rx = bus.subscribe(TOPIC_ACK);

        // Simulate host/buggy handler: receive action and emit ack.
        let bus_for_handler = Arc::clone(&bus);
        tokio::spawn(async move {
            if let Ok(msg) = action_rx.recv().await {
                if let Ok(text) = std::str::from_utf8(&msg.payload) {
                    if let Ok(action) = parse_action_payload(text) {
                        let ack = ActionAck {
                            id: action.id.clone(),
                            status: beach_buggy::AckStatus::Ok,
                            applied_at: std::time::SystemTime::now(),
                            latency_ms: None,
                            error_code: None,
                            error_message: None,
                        };
                        let _ = bus_for_handler
                            .publish(TOPIC_ACK, serde_json::to_vec(&ack).unwrap().into());
                    }
                }
            }
        });

        let action = ActionCommand {
            id: "test-action".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({ "bytes": "ping" }),
            expires_at: None,
        };
        bus.publish(TOPIC_ACTION, serde_json::to_vec(&action).unwrap().into())
            .expect("publish action");

        let msg = timeout(Duration::from_secs(1), ack_rx.recv())
            .await
            .expect("ack timeout")
            .expect("ack msg");
        let ack: ActionAck = serde_json::from_slice(&msg.payload).expect("parse ack payload");
        assert_eq!(ack.id, "test-action");
    }
}

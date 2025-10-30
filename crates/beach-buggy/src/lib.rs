//! Beach Buggy: high-performance harness runtime shared by Beach clients.
//!
//! This crate encapsulates the harness-side logic for:
//! - registering sessions with the Beach Manager control plane
//! - streaming terminal / GUI diffs in an efficient, structured format
//!
//! - consuming controller actions, acknowledging results, and reacting to lease changes
//! - emitting health/heartbeat information so the manager can enforce zero-trust policy
//!
//! The goal is to provide a reusable crate that both `apps/beach` (terminal) and
//! `beach-cabana` (GUI) can embed while keeping their platform specifics thin.

use async_trait::async_trait;
use futures::stream::Stream;
use futures::{stream, StreamExt};
use reqwest::Url;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{
    sync::{broadcast, Mutex},
    time::{sleep, timeout},
};
use tracing::{debug, error, info, trace, warn};

use crate::fast_path::{parse_fast_path_endpoints, FastPathClient, FastPathConnection};

pub mod fast_path;

/// Convenient result alias used throughout the harness.
pub type HarnessResult<T> = Result<T, HarnessError>;

/// Error type surfaced by Beach Buggy helpers.
#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
    #[error("controller mismatch")]
    ControllerMismatch,
}

/// Struct used when registering the harness with Beach Manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSessionRequest {
    pub session_id: String,
    pub private_beach_id: String,
    pub harness_type: HarnessType,
    pub capabilities: Vec<String>,
    pub location_hint: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_passcode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterSessionResponse {
    pub harness_id: String,
    pub controller_token: Option<String>,
    pub lease_ttl_ms: u64,
    pub state_cache_url: Option<String>,
    pub transport_hints: HashMap<String, serde_json::Value>,
}

impl Default for RegisterSessionResponse {
    fn default() -> Self {
        Self {
            harness_id: String::new(),
            controller_token: None,
            lease_ttl_ms: 0,
            state_cache_url: None,
            transport_hints: HashMap::new(),
        }
    }
}

/// Harness classification. More variants can be added as we support new capture types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HarnessType {
    TerminalShim,
    CabanaAdapter,
    RemoteWidget,
    ServiceProxy,
    Custom,
}

/// Diff payload emitted to the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDiff {
    pub sequence: u64,
    pub emitted_at: SystemTime,
    pub payload: serde_json::Value,
}

/// Command instruction delivered by the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCommand {
    pub id: String,
    pub action_type: String,
    pub payload: serde_json::Value,
    pub expires_at: Option<SystemTime>,
}

/// Acknowledgement payload returned to the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionAck {
    pub id: String,
    pub status: AckStatus,
    pub applied_at: SystemTime,
    pub latency_ms: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckStatus {
    Ok,
    Rejected,
    Expired,
    Preempted,
}

/// Heartbeat payload describing current harness health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthHeartbeat {
    pub queue_depth: usize,
    pub cpu_load: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub degraded: bool,
    pub warnings: Vec<String>,
}

/// Manager notification that controller token changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerNotification {
    pub controller_token: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControllerUpdateCadence {
    Fast,
    #[default]
    Balanced,
    Slow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingTransportKind {
    FastPath,
    HttpFallback,
    Pending,
}

impl Default for PairingTransportKind {
    fn default() -> Self {
        PairingTransportKind::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PairingTransportStatus {
    pub transport: PairingTransportKind,
    #[serde(default)]
    pub last_event_ms: Option<i64>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControllerPairing {
    pub pairing_id: String,
    pub controller_session_id: String,
    pub child_session_id: String,
    #[serde(default)]
    pub prompt_template: Option<String>,
    pub update_cadence: ControllerUpdateCadence,
    #[serde(default)]
    pub transport_status: Option<PairingTransportStatus>,
    #[serde(default)]
    pub created_at_ms: Option<i64>,
    #[serde(default)]
    pub updated_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPairingAction {
    Added,
    Updated,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControllerPairingEvent {
    pub controller_session_id: String,
    pub child_session_id: String,
    pub action: ControllerPairingAction,
    #[serde(default)]
    pub pairing: Option<ControllerPairing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerLeaseRenewal {
    pub controller_token: String,
    pub expires_at_ms: i64,
}

pub type ControllerPairingStream =
    Pin<Box<dyn Stream<Item = HarnessResult<ControllerPairingEvent>> + Send>>;

/// Representation of a terminal frame we can diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StyledCell {
    pub ch: char,
    pub style: u32,
}

/// Serialized style definition for terminal snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StyleDefinition {
    pub id: u32,
    pub fg: u32,
    pub bg: u32,
    pub attrs: u32,
}

/// Representation of a terminal frame we can diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalFrame {
    pub lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub styled_lines: Option<Vec<Vec<StyledCell>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub styles: Option<Vec<StyleDefinition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
    pub cursor: Option<CursorPosition>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CursorPosition {
    pub row: usize,
    pub col: usize,
}

/// Representation of a Cabana GUI frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CabanaFrame {
    pub fps: f32,
    pub cursor: Option<(f32, f32)>,
    pub mouse_buttons: Vec<String>,
    pub windows: Vec<WindowRegion>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowRegion {
    pub id: String,
    pub title: Option<String>,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Abstract transport that the harness uses to communicate with Beach Manager.
#[async_trait]
pub trait ManagerTransport: Send + Sync + 'static {
    async fn register_session(
        &self,
        request: RegisterSessionRequest,
    ) -> HarnessResult<RegisterSessionResponse>;

    async fn send_state(&self, session_id: &str, diff: StateDiff) -> HarnessResult<()>;

    async fn receive_actions(&self, session_id: &str) -> HarnessResult<Vec<ActionCommand>>;

    async fn ack_actions(&self, session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()>;

    async fn signal_health(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> HarnessResult<()>;
}

#[async_trait]
pub trait ControllerTransport: Send + Sync + 'static {
    async fn list_controller_pairings(
        &self,
        controller_session_id: &str,
    ) -> HarnessResult<Vec<ControllerPairing>>;

    async fn stream_controller_pairings(
        &self,
        controller_session_id: &str,
    ) -> HarnessResult<ControllerPairingStream>;

    async fn renew_controller_lease(
        &self,
        session_id: &str,
        ttl_ms: Option<u64>,
    ) -> HarnessResult<ControllerLeaseRenewal>;
}

/// Configuration for instantiating a harness.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    pub session_id: String,
    pub private_beach_id: String,
    pub harness_type: HarnessType,
    pub capabilities: Vec<String>,
    pub location_hint: Option<String>,
    pub version: String,
    pub viewer_passcode: Option<String>,
}

impl HarnessConfig {
    pub fn into_register_request(
        self,
        metadata: Option<serde_json::Value>,
    ) -> RegisterSessionRequest {
        RegisterSessionRequest {
            session_id: self.session_id,
            private_beach_id: self.private_beach_id,
            harness_type: self.harness_type,
            capabilities: self.capabilities,
            location_hint: self.location_hint,
            metadata,
            version: self.version,
            viewer_passcode: self.viewer_passcode,
        }
    }
}

struct HarnessState {
    controller_token: Option<String>,
    last_terminal: Option<TerminalFrame>,
    last_cabana: Option<CabanaFrame>,
    pending_actions: Vec<ActionCommand>,
}

impl HarnessState {
    fn new() -> Self {
        Self {
            controller_token: None,
            last_terminal: None,
            last_cabana: None,
            pending_actions: Vec::new(),
        }
    }

    fn queue_depth(&self) -> usize {
        self.pending_actions.len()
    }
}

/// Core harness struct combining configuration, transport, and diff helpers.
pub struct SessionHarness<T: ManagerTransport> {
    config: Arc<HarnessConfig>,
    transport: Arc<T>,
    state: Arc<Mutex<HarnessState>>,
    seq: AtomicU64,
}

impl<T: ManagerTransport> SessionHarness<T> {
    pub fn new(config: HarnessConfig, transport: T) -> Self {
        Self {
            config: Arc::new(config),
            transport: Arc::new(transport),
            state: Arc::new(Mutex::new(HarnessState::new())),
            seq: AtomicU64::new(0),
        }
    }

    /// Registers the harness with the manager and updates internal controller token.
    pub async fn register(
        &self,
        metadata: Option<serde_json::Value>,
    ) -> HarnessResult<RegisterSessionResponse> {
        let request = self.config.as_ref().clone().into_register_request(metadata);
        let response = self
            .transport
            .register_session(request)
            .await
            .map_err(|e| HarnessError::Transport(format!("{e}")))?;

        {
            let mut state = self.state.lock().await;
            state.controller_token = response.controller_token.clone();
        }

        Ok(response)
    }

    /// Pushes a terminal frame; returns generated diff for consumers/tests.
    pub async fn push_terminal_frame(&self, frame: TerminalFrame) -> HarnessResult<StateDiff> {
        let payload = build_terminal_payload(&frame);
        let diff = self.build_diff(payload);

        {
            let mut state = self.state.lock().await;
            state.last_terminal = Some(frame);
        }

        self.transport
            .send_state(&self.config.session_id, diff.clone())
            .await?;
        Ok(diff)
    }

    /// Pushes a Cabana GUI frame.
    pub async fn push_cabana_frame(&self, frame: CabanaFrame) -> HarnessResult<StateDiff> {
        let payload = serde_json::json!({
            "type": "cabana_frame",
            "fps": frame.fps,
            "cursor": frame.cursor,
            "mouse_buttons": frame.mouse_buttons,
            "windows": frame.windows,
        });
        let diff = self.build_diff(payload);

        {
            let mut state = self.state.lock().await;
            state.last_cabana = Some(frame);
        }

        self.transport
            .send_state(&self.config.session_id, diff.clone())
            .await?;
        Ok(diff)
    }

    /// Polls for new actions from the manager and tracks them locally until acked.
    pub async fn poll_actions(&self) -> HarnessResult<Vec<ActionCommand>> {
        let commands = self
            .transport
            .receive_actions(&self.config.session_id)
            .await?;

        if !commands.is_empty() {
            let mut state = self.state.lock().await;
            state.pending_actions.extend(commands.clone());
            trace!(
                pending = state.pending_actions.len(),
                "queued new actions for session {}",
                self.config.session_id
            );
        }

        Ok(commands)
    }

    /// Marks an action as completed and sends acknowledgement to the manager.
    pub async fn ack_action(
        &self,
        command_id: &str,
        status: AckStatus,
        error: Option<&str>,
    ) -> HarnessResult<()> {
        let mut removed = None;
        {
            let mut state = self.state.lock().await;
            if let Some(pos) = state
                .pending_actions
                .iter()
                .position(|cmd| cmd.id == command_id)
            {
                removed = Some(state.pending_actions.remove(pos));
            }
        }

        if removed.is_none() {
            return Err(HarnessError::InvalidState("action not found in queue"));
        }

        let ack = ActionAck {
            id: command_id.to_owned(),
            status,
            applied_at: SystemTime::now(),
            latency_ms: None,
            error_code: error.map(ToOwned::to_owned),
            error_message: error.map(ToOwned::to_owned),
        };

        self.transport
            .ack_actions(&self.config.session_id, vec![ack])
            .await?;
        Ok(())
    }

    /// Emits heartbeat to manager with optional overrides (e.g., custom queue depth).
    pub async fn signal_health(&self, mut heartbeat: HealthHeartbeat) -> HarnessResult<()> {
        if heartbeat.queue_depth == usize::MAX {
            let state = self.state.lock().await;
            heartbeat.queue_depth = state.queue_depth();
        }

        self.transport
            .signal_health(&self.config.session_id, heartbeat)
            .await
    }

    /// Returns the current controller token if one is held.
    pub async fn controller_token(&self) -> Option<String> {
        self.state.lock().await.controller_token.clone()
    }

    /// Called when manager revokes controller lease or hands it to another entity.
    pub async fn handle_controller_notification(
        &self,
        notification: ControllerNotification,
    ) -> HarnessResult<()> {
        let mut to_ack = Vec::new();
        {
            let mut state = self.state.lock().await;
            if state.controller_token != notification.controller_token {
                if !state.pending_actions.is_empty() {
                    warn!(
                        "controller changed for session {}; preempting {} actions",
                        self.config.session_id,
                        state.pending_actions.len()
                    );
                    let now = SystemTime::now();
                    to_ack = state
                        .pending_actions
                        .drain(..)
                        .map(|cmd| ActionAck {
                            id: cmd.id,
                            status: AckStatus::Preempted,
                            applied_at: now,
                            latency_ms: None,
                            error_code: Some("controller_preempted".into()),
                            error_message: notification.reason.clone(),
                        })
                        .collect();
                }
                state.controller_token = notification.controller_token.clone();
            }
        }

        if !to_ack.is_empty() {
            self.transport
                .ack_actions(&self.config.session_id, to_ack)
                .await?;
        }

        Ok(())
    }

    fn build_diff(&self, payload: serde_json::Value) -> StateDiff {
        let sequence = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        StateDiff {
            sequence,
            emitted_at: SystemTime::now(),
            payload,
        }
    }
}

impl<T> SessionHarness<T>
where
    T: ManagerTransport + ControllerTransport + Send + Sync + 'static,
{
    /// Spawns background tasks that keep the controller lease fresh and
    /// react to pairing updates emitted by the manager.
    pub fn spawn_controller_runtime(
        &self,
        initial_lease_ttl_ms: u64,
    ) -> tokio::task::JoinHandle<()> {
        let transport = self.transport.clone();
        let state = self.state.clone();
        let config = self.config.clone();
        tokio::spawn(async move {
            controller_runtime::run(transport, state, config, initial_lease_ttl_ms).await;
        })
    }
}

fn build_terminal_payload(frame: &TerminalFrame) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "type".into(),
        serde_json::Value::String("terminal_full".into()),
    );
    payload.insert(
        "lines".into(),
        serde_json::to_value(&frame.lines).expect("serialize terminal lines"),
    );
    if let Some(cursor) = frame.cursor {
        payload.insert(
            "cursor".into(),
            serde_json::json!({ "row": cursor.row, "col": cursor.col }),
        );
    }
    if let Some(styled_lines) = &frame.styled_lines {
        payload.insert(
            "styled_lines".into(),
            serde_json::to_value(styled_lines).expect("serialize styled lines"),
        );
    }
    if let Some(styles) = &frame.styles {
        payload.insert(
            "styles".into(),
            serde_json::to_value(styles).expect("serialize style definitions"),
        );
    }
    if let Some(cols) = frame.cols {
        payload.insert("cols".into(), serde_json::json!(cols));
    }
    if let Some(rows) = frame.rows {
        payload.insert("rows".into(), serde_json::json!(rows));
    }
    serde_json::Value::Object(payload)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

mod controller_runtime {
    use super::*;
    use std::collections::HashMap;

    const MIN_RENEW_WINDOW_MS: u64 = 1_000;
    const RENEW_FRACTION: f32 = 0.5;
    const STREAM_RETRY_DELAY: Duration = Duration::from_secs(5);

    pub async fn run<T>(
        transport: Arc<T>,
        state: Arc<Mutex<HarnessState>>,
        config: Arc<HarnessConfig>,
        initial_lease_ttl_ms: u64,
    ) where
        T: ControllerTransport + Send + Sync + 'static,
    {
        let lease_transport = transport.clone();
        let lease_state = state.clone();
        let session_id = config.session_id.clone();
        let private_beach_id = config.private_beach_id.clone();

        let lease_task = tokio::spawn(async move {
            lease_loop(
                lease_transport,
                lease_state,
                session_id,
                initial_lease_ttl_ms,
            )
            .await;
        });

        let pairing_task = tokio::spawn(async move {
            pairing_loop(transport, config, private_beach_id).await;
        });

        let _ = tokio::join!(lease_task, pairing_task);
    }

    async fn lease_loop<T>(
        transport: Arc<T>,
        state: Arc<Mutex<HarnessState>>,
        session_id: String,
        mut ttl_ms: u64,
    ) where
        T: ControllerTransport + Send + Sync + 'static,
    {
        if ttl_ms == 0 {
            ttl_ms = 30_000;
        }

        loop {
            let window = ((ttl_ms as f32) * RENEW_FRACTION).max(MIN_RENEW_WINDOW_MS as f32) as u64;
            sleep(Duration::from_millis(window)).await;

            match transport
                .renew_controller_lease(&session_id, Some(ttl_ms))
                .await
            {
                Ok(lease) => {
                    {
                        let mut guard = state.lock().await;
                        guard.controller_token = Some(lease.controller_token.clone());
                    }

                    let expires_in = lease
                        .expires_at_ms
                        .saturating_sub(now_millis())
                        .max(MIN_RENEW_WINDOW_MS as i64)
                        as u64;
                    ttl_ms = expires_in;
                    debug!(
                        target = "controller_pairing",
                        session_id = %session_id,
                        expires_in_ms = expires_in,
                        "controller lease renewed"
                    );
                }
                Err(err) => {
                    warn!(
                        target = "controller_pairing",
                        session_id = %session_id,
                        error = %err,
                        "controller lease renewal failed; retrying"
                    );
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn pairing_loop<T>(
        transport: Arc<T>,
        config: Arc<HarnessConfig>,
        private_beach_id: String,
    ) where
        T: ControllerTransport + Send + Sync + 'static,
    {
        let controller_session_id = config.session_id.clone();
        let mut known: HashMap<String, ControllerPairing> = HashMap::new();

        if let Ok(snapshot) = transport
            .list_controller_pairings(&controller_session_id)
            .await
        {
            apply_snapshot(
                &controller_session_id,
                &private_beach_id,
                &mut known,
                snapshot,
            );
        }

        loop {
            match transport
                .stream_controller_pairings(&controller_session_id)
                .await
            {
                Ok(mut stream) => {
                    info!(
                        target = "controller_pairing",
                        controller_session_id = %controller_session_id,
                        "controller pairing stream connected"
                    );
                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(ev) => {
                                handle_event(
                                    &transport,
                                    &controller_session_id,
                                    &private_beach_id,
                                    &mut known,
                                    ev,
                                )
                                .await;
                            }
                            Err(err) => {
                                warn!(
                                    target = "controller_pairing",
                                    controller_session_id = %controller_session_id,
                                    error = %err,
                                    "controller pairing stream error"
                                );
                                break;
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(
                        target = "controller_pairing",
                        controller_session_id = %controller_session_id,
                        error = %err,
                        "controller pairing stream unavailable"
                    );
                }
            }

            if let Ok(snapshot) = transport
                .list_controller_pairings(&controller_session_id)
                .await
            {
                apply_snapshot(
                    &controller_session_id,
                    &private_beach_id,
                    &mut known,
                    snapshot,
                );
            }

            sleep(STREAM_RETRY_DELAY).await;
        }
    }

    async fn handle_event<T>(
        transport: &Arc<T>,
        controller_session_id: &str,
        private_beach_id: &str,
        known: &mut HashMap<String, ControllerPairing>,
        event: ControllerPairingEvent,
    ) where
        T: ControllerTransport + Send + Sync + 'static,
    {
        match event.action {
            action @ ControllerPairingAction::Added | action @ ControllerPairingAction::Updated => {
                if let Some(pairing) = event.pairing {
                    upsert_pairing(
                        controller_session_id,
                        private_beach_id,
                        known,
                        pairing,
                        action,
                    );
                } else if let Ok(snapshot) = transport
                    .list_controller_pairings(controller_session_id)
                    .await
                {
                    apply_snapshot(controller_session_id, private_beach_id, known, snapshot);
                }
            }
            ControllerPairingAction::Removed => {
                known.remove(&event.child_session_id);
                info!(
                    target = "controller_pairing",
                    controller_session_id,
                    private_beach_id,
                    child_session_id = %event.child_session_id,
                    "controller pairing removed"
                );
            }
        }
    }

    fn upsert_pairing(
        controller_session_id: &str,
        private_beach_id: &str,
        known: &mut HashMap<String, ControllerPairing>,
        pairing: ControllerPairing,
        action: ControllerPairingAction,
    ) {
        let child = pairing.child_session_id.clone();
        let entry = known.insert(child.clone(), pairing.clone());
        let log_action = if entry.is_some() {
            ControllerPairingAction::Updated
        } else {
            action
        };
        log_pairing(
            controller_session_id,
            private_beach_id,
            &pairing,
            log_action,
        );
    }

    fn apply_snapshot(
        controller_session_id: &str,
        private_beach_id: &str,
        known: &mut HashMap<String, ControllerPairing>,
        snapshot: Vec<ControllerPairing>,
    ) {
        let mut next = HashMap::new();
        for pairing in snapshot {
            let child = pairing.child_session_id.clone();
            if let Some(existing) = known.get(&child) {
                if existing != &pairing {
                    log_pairing(
                        controller_session_id,
                        private_beach_id,
                        &pairing,
                        ControllerPairingAction::Updated,
                    );
                }
            } else {
                log_pairing(
                    controller_session_id,
                    private_beach_id,
                    &pairing,
                    ControllerPairingAction::Added,
                );
            }
            next.insert(child, pairing);
        }

        for removed in known
            .keys()
            .filter(|child| !next.contains_key(*child))
            .cloned()
            .collect::<Vec<_>>()
        {
            info!(
                target = "controller_pairing",
                controller_session_id,
                private_beach_id,
                child_session_id = %removed,
                "controller pairing removed"
            );
        }

        *known = next;
    }

    fn log_pairing(
        controller_session_id: &str,
        private_beach_id: &str,
        pairing: &ControllerPairing,
        action: ControllerPairingAction,
    ) {
        let transport = pairing
            .transport_status
            .as_ref()
            .map(|status| status.transport)
            .unwrap_or_default();
        let transport_latency = pairing
            .transport_status
            .as_ref()
            .and_then(|status| status.latency_ms);
        let transport_error = pairing
            .transport_status
            .as_ref()
            .and_then(|status| status.last_error.as_deref());
        info!(
            target = "controller_pairing",
            controller_session_id,
            private_beach_id,
            child_session_id = %pairing.child_session_id,
            update_cadence = ?pairing.update_cadence,
            prompt_template = ?pairing.prompt_template,
            transport = ?transport,
            transport_latency = ?transport_latency,
            transport_error = ?transport_error,
            action = ?action,
            "controller pairing update"
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;

        fn sample_pairing(
            controller: &str,
            child: &str,
            status: Option<PairingTransportStatus>,
        ) -> ControllerPairing {
            ControllerPairing {
                pairing_id: format!("{controller}:{child}"),
                controller_session_id: controller.into(),
                child_session_id: child.into(),
                prompt_template: Some("example prompt".into()),
                update_cadence: ControllerUpdateCadence::Balanced,
                transport_status: status,
                created_at_ms: Some(1),
                updated_at_ms: Some(1),
            }
        }

        #[test]
        fn apply_snapshot_persists_transport_status() {
            let mut known = HashMap::new();
            let status = PairingTransportStatus {
                transport: PairingTransportKind::FastPath,
                last_event_ms: Some(42),
                latency_ms: Some(7),
                last_error: None,
            };
            let pairing = sample_pairing("controller-1", "child-1", Some(status.clone()));

            apply_snapshot("controller-1", "pb-test", &mut known, vec![pairing]);

            let stored = known
                .get("child-1")
                .expect("pairing should be stored after snapshot");
            assert_eq!(stored.transport_status, Some(status));
        }

        #[test]
        fn upsert_pairing_updates_existing_status() {
            let mut known = HashMap::new();
            let initial = sample_pairing("controller-1", "child-1", None);
            apply_snapshot("controller-1", "pb-test", &mut known, vec![initial]);

            let updated_status = PairingTransportStatus {
                transport: PairingTransportKind::HttpFallback,
                last_event_ms: Some(99),
                latency_ms: Some(15),
                last_error: Some("fast_path_unavailable".into()),
            };
            let updated = sample_pairing("controller-1", "child-1", Some(updated_status.clone()));

            upsert_pairing(
                "controller-1",
                "pb-test",
                &mut known,
                updated,
                ControllerPairingAction::Updated,
            );

            let stored = known
                .get("child-1")
                .expect("pairing should remain after update");
            assert_eq!(stored.transport_status, Some(updated_status));
        }
    }
}

// ----------------------------------------------------------------------------- //
// HTTP transport implementation
// ----------------------------------------------------------------------------- //

/// Provides bearer tokens for authenticating with Beach Manager.
#[async_trait]
pub trait TokenProvider: Send + Sync + 'static {
    async fn token(&self) -> HarnessResult<String>;
}

/// Simple static token provider.
#[derive(Clone)]
pub struct StaticTokenProvider {
    token: String,
}

impl StaticTokenProvider {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

#[async_trait]
impl TokenProvider for StaticTokenProvider {
    async fn token(&self) -> HarnessResult<String> {
        Ok(self.token.clone())
    }
}

/// HTTP implementation of the manager transport.
pub struct HttpTransport<P: TokenProvider> {
    client: reqwest::Client,
    base_url: Url,
    token_provider: P,
    fast_path: Arc<Mutex<FastPathState>>,
}

#[derive(Debug, Deserialize)]
struct ControllerLeaseResponse {
    controller_token: String,
    expires_at_ms: i64,
}

#[derive(Default)]
struct FastPathState {
    client: Option<FastPathClient>,
    connection: Option<FastPathConnection>,
    actions_rx: Option<broadcast::Receiver<ActionCommand>>,
}

impl<P: TokenProvider> HttpTransport<P> {
    pub fn new(base_url: impl AsRef<str>, token_provider: P) -> HarnessResult<Self> {
        let mut base = Url::parse(base_url.as_ref())
            .map_err(|e| HarnessError::Transport(format!("invalid base url: {e}")))?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            client: reqwest::Client::new(),
            base_url: base,
            token_provider,
            fast_path: Arc::new(Mutex::new(FastPathState::default())),
        })
    }

    async fn configure_fast_path(&self, hints: &HashMap<String, serde_json::Value>) {
        match parse_fast_path_endpoints(self.base_url(), hints) {
            Ok(Some(endpoints)) => {
                let client = FastPathClient::new(endpoints.clone());
                {
                    let mut state = self.fast_path.lock().await;
                    state.client = Some(client.clone());
                }
                if let Err(err) = self.establish_fast_path(client).await {
                    warn!(
                        target = "fast_path",
                        error = %err,
                        "failed to establish fast-path connection; continuing with HTTP transport"
                    );
                }
            }
            Ok(None) => {
                self.clear_fast_path().await;
            }
            Err(err) => {
                warn!(
                    target = "fast_path",
                    error = %err,
                    "invalid fast-path hint returned by manager; ignoring"
                );
            }
        }
    }

    async fn establish_fast_path(
        &self,
        client: FastPathClient,
    ) -> HarnessResult<FastPathConnection> {
        let token = self.token_provider.token().await?;
        let connection = client.connect(&token).await?;
        let receiver = connection.subscribe_actions();
        {
            let mut state = self.fast_path.lock().await;
            state.connection = Some(connection.clone());
            state.actions_rx = Some(receiver);
            state.client = Some(client);
        }
        info!(target = "fast_path", "fast-path connection established");
        Ok(connection)
    }

    async fn ensure_fast_path_connection(&self) -> Option<FastPathConnection> {
        let client = {
            let state = self.fast_path.lock().await;
            if let Some(conn) = &state.connection {
                return Some(conn.clone());
            }
            state.client.clone()
        };

        if let Some(client) = client {
            match self.establish_fast_path(client.clone()).await {
                Ok(conn) => Some(conn),
                Err(err) => {
                    warn!(
                        target = "fast_path",
                        error = %err,
                        "failed to connect fast-path; falling back to HTTP"
                    );
                    None
                }
            }
        } else {
            None
        }
    }

    async fn drop_fast_path_connection(&self) {
        let connection = {
            let mut state = self.fast_path.lock().await;
            state.actions_rx = None;
            state.connection.take()
        };
        if let Some(conn) = connection {
            if let Err(err) = conn.peer.close().await {
                warn!(
                    target = "fast_path",
                    error = %err,
                    "failed to close fast-path peer connection"
                );
            }
        }
    }

    async fn clear_fast_path(&self) {
        let connection = {
            let mut state = self.fast_path.lock().await;
            state.client = None;
            state.actions_rx = None;
            state.connection.take()
        };
        if let Some(conn) = connection {
            if let Err(err) = conn.peer.close().await {
                warn!(
                    target = "fast_path",
                    error = %err,
                    "failed to close fast-path peer connection"
                );
            }
        }
    }

    async fn take_actions_receiver(&self) -> Option<broadcast::Receiver<ActionCommand>> {
        let mut state = self.fast_path.lock().await;
        if state.connection.is_some() {
            state.actions_rx.take()
        } else {
            None
        }
    }

    async fn store_actions_receiver(&self, rx: broadcast::Receiver<ActionCommand>) {
        let mut state = self.fast_path.lock().await;
        if state.connection.is_some() {
            state.actions_rx = Some(rx);
        }
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn url(&self, path: &str) -> HarnessResult<Url> {
        self.base_url
            .join(path)
            .map_err(|e| HarnessError::Transport(format!("invalid path {path}: {e}")))
    }

    async fn request_with_token(
        &self,
        req: reqwest::RequestBuilder,
    ) -> HarnessResult<reqwest::Response> {
        let token = self.token_provider.token().await?;
        let response = req
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| HarnessError::Transport(format!("http send error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read body>".into());
            return Err(HarnessError::Transport(format!(
                "unexpected status {} body={}",
                status, body
            )));
        }
        Ok(response)
    }
}

#[async_trait]
impl<P: TokenProvider> ManagerTransport for HttpTransport<P> {
    async fn register_session(
        &self,
        request: RegisterSessionRequest,
    ) -> HarnessResult<RegisterSessionResponse> {
        let url = self.url("sessions/register")?;
        let resp = self
            .request_with_token(self.client.post(url).json(&request))
            .await?;
        let register_response = resp
            .json::<RegisterSessionResponse>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode register response: {e}")))?;

        self.configure_fast_path(&register_response.transport_hints)
            .await;

        Ok(register_response)
    }

    async fn send_state(&self, session_id: &str, diff: StateDiff) -> HarnessResult<()> {
        if let Some(conn) = self.ensure_fast_path_connection().await {
            if let Err(err) = conn.send_state(&diff).await {
                warn!(
                    target = "fast_path",
                    error = %err,
                    "fast-path state send failed; reverting to HTTP"
                );
                self.drop_fast_path_connection().await;
            } else {
                return Ok(());
            }
        }

        let url = self.url(&format!("sessions/{session_id}/state"))?;
        self.request_with_token(self.client.post(url).json(&diff))
            .await?;
        Ok(())
    }

    async fn receive_actions(&self, session_id: &str) -> HarnessResult<Vec<ActionCommand>> {
        if self.ensure_fast_path_connection().await.is_some() {
            if let Some(mut rx) = self.take_actions_receiver().await {
                let mut commands = Vec::new();
                let mut keep_receiver = true;

                loop {
                    use tokio::sync::broadcast::error::TryRecvError;
                    match rx.try_recv() {
                        Ok(cmd) => commands.push(cmd),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Lagged(skipped)) => {
                            warn!(
                                target = "fast_path",
                                skipped, "fast-path action channel lagged; dropping connection"
                            );
                            keep_receiver = false;
                            self.drop_fast_path_connection().await;
                            break;
                        }
                        Err(TryRecvError::Closed) => {
                            keep_receiver = false;
                            self.drop_fast_path_connection().await;
                            break;
                        }
                    }
                }

                if commands.is_empty() && keep_receiver {
                    use tokio::sync::broadcast::error::RecvError;
                    match timeout(std::time::Duration::from_millis(50), rx.recv()).await {
                        Ok(Ok(cmd)) => {
                            commands.push(cmd);
                            while let Ok(cmd2) = rx.try_recv() {
                                commands.push(cmd2);
                            }
                        }
                        Ok(Err(RecvError::Lagged(skipped))) => {
                            warn!(
                                target = "fast_path",
                                skipped, "fast-path action channel lagged; dropping connection"
                            );
                            keep_receiver = false;
                            self.drop_fast_path_connection().await;
                        }
                        Ok(Err(RecvError::Closed)) => {
                            keep_receiver = false;
                            self.drop_fast_path_connection().await;
                        }
                        Err(_) => {
                            // timeout waiting for actions; fall back to HTTP below
                        }
                    }
                }

                if keep_receiver {
                    self.store_actions_receiver(rx).await;
                }

                if !commands.is_empty() {
                    return Ok(commands);
                }
            }
        }

        let url = self.url(&format!("sessions/{session_id}/actions/poll"))?;
        let resp = self.request_with_token(self.client.get(url)).await?;
        resp.json::<Vec<ActionCommand>>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode actions: {e}")))
    }

    async fn ack_actions(&self, session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()> {
        if let Some(conn) = self.ensure_fast_path_connection().await {
            if let Err(err) = conn.send_acks(&acks).await {
                warn!(
                    target = "fast_path",
                    error = %err,
                    "fast-path ack send failed; reverting to HTTP"
                );
                self.drop_fast_path_connection().await;
            } else {
                return Ok(());
            }
        }

        let url = self.url(&format!("sessions/{session_id}/actions/ack"))?;
        self.request_with_token(self.client.post(url).json(&acks))
            .await?;
        Ok(())
    }

    async fn signal_health(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> HarnessResult<()> {
        let url = self.url(&format!("sessions/{session_id}/health"))?;
        self.request_with_token(self.client.post(url).json(&heartbeat))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl<P: TokenProvider> ControllerTransport for HttpTransport<P> {
    async fn list_controller_pairings(
        &self,
        controller_session_id: &str,
    ) -> HarnessResult<Vec<ControllerPairing>> {
        let url = self.url(&format!("sessions/{controller_session_id}/controllers"))?;
        let resp = self.request_with_token(self.client.get(url)).await?;
        resp.json::<Vec<ControllerPairing>>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode controller pairings: {e}")))
    }

    async fn stream_controller_pairings(
        &self,
        controller_session_id: &str,
    ) -> HarnessResult<ControllerPairingStream> {
        let url = self.url(&format!(
            "sessions/{controller_session_id}/controllers/stream"
        ))?;
        let token = self.token_provider.token().await?;
        let request = self.client.get(url).bearer_auth(token);
        let source = EventSource::new(request).map_err(|e| {
            HarnessError::Transport(format!("connect controller pairing stream failed: {e}"))
        })?;

        let stream = source.filter_map(|event| async move {
            match event {
                Ok(Event::Open) => {
                    trace!(target = "controller_pairing", "pairing stream opened");
                    None
                }
                Ok(Event::Message(msg)) => {
                    if msg.event == "controller_pairing" {
                        Some(
                            serde_json::from_str::<ControllerPairingEvent>(&msg.data).map_err(
                                |err| {
                                    HarnessError::Transport(format!(
                                        "decode controller pairing event: {err}"
                                    ))
                                },
                            ),
                        )
                    } else {
                        None
                    }
                }
                Err(err) => Some(Err(HarnessError::Transport(format!(
                    "controller pairing stream error: {err}"
                )))),
            }
        });

        Ok(Box::pin(stream))
    }

    async fn renew_controller_lease(
        &self,
        session_id: &str,
        ttl_ms: Option<u64>,
    ) -> HarnessResult<ControllerLeaseRenewal> {
        let url = self.url(&format!("sessions/{session_id}/controller/lease"))?;
        let mut body = serde_json::Map::new();
        if let Some(ttl) = ttl_ms {
            body.insert("ttl_ms".into(), serde_json::Value::from(ttl));
        }
        let resp = self
            .request_with_token(self.client.post(url).json(&body))
            .await?;
        let lease = resp
            .json::<ControllerLeaseResponse>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode controller lease: {e}")))?;
        Ok(ControllerLeaseRenewal {
            controller_token: lease.controller_token,
            expires_at_ms: lease.expires_at_ms,
        })
    }
}

// ----------------------------------------------------------------------------- //
// In-memory transport for tests / prototyping
// ----------------------------------------------------------------------------- //

#[derive(Default, Clone)]
pub struct InMemoryTransport {
    inner: Arc<Mutex<InMemoryState>>,
}

#[derive(Default)]
struct InMemoryState {
    register_requests: Vec<RegisterSessionRequest>,
    register_response: RegisterSessionResponse,
    diffs: Vec<StateDiff>,
    actions: Vec<Vec<ActionCommand>>,
    acks: Vec<Vec<ActionAck>>,
    health: Vec<HealthHeartbeat>,
    controller_pairings: Vec<ControllerPairing>,
}

impl InMemoryTransport {
    pub fn with_response(response: RegisterSessionResponse) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InMemoryState {
                register_response: response,
                ..Default::default()
            })),
        }
    }

    pub async fn push_actions(&self, cmds: Vec<ActionCommand>) {
        let mut state = self.inner.lock().await;
        state.actions.push(cmds);
    }

    pub async fn diffs(&self) -> Vec<StateDiff> {
        self.inner.lock().await.diffs.clone()
    }

    pub async fn acks(&self) -> Vec<Vec<ActionAck>> {
        self.inner.lock().await.acks.clone()
    }

    pub async fn health(&self) -> Vec<HealthHeartbeat> {
        self.inner.lock().await.health.clone()
    }

    pub async fn set_pairings(&self, pairings: Vec<ControllerPairing>) {
        self.inner.lock().await.controller_pairings = pairings;
    }
}

#[async_trait]
impl ManagerTransport for InMemoryTransport {
    async fn register_session(
        &self,
        request: RegisterSessionRequest,
    ) -> HarnessResult<RegisterSessionResponse> {
        let mut state = self.inner.lock().await;
        state.register_requests.push(request);
        Ok(state.register_response.clone())
    }

    async fn send_state(&self, _session_id: &str, diff: StateDiff) -> HarnessResult<()> {
        let mut state = self.inner.lock().await;
        state.diffs.push(diff);
        Ok(())
    }

    async fn receive_actions(&self, _session_id: &str) -> HarnessResult<Vec<ActionCommand>> {
        let mut state = self.inner.lock().await;
        if !state.actions.is_empty() {
            Ok(state.actions.remove(0))
        } else {
            Ok(Vec::new())
        }
    }

    async fn ack_actions(&self, _session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()> {
        let mut state = self.inner.lock().await;
        state.acks.push(acks);
        Ok(())
    }

    async fn signal_health(
        &self,
        _session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> HarnessResult<()> {
        let mut state = self.inner.lock().await;
        state.health.push(heartbeat);
        Ok(())
    }
}

#[async_trait]
impl ControllerTransport for InMemoryTransport {
    async fn list_controller_pairings(
        &self,
        _controller_session_id: &str,
    ) -> HarnessResult<Vec<ControllerPairing>> {
        Ok(self.inner.lock().await.controller_pairings.clone())
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
    ) -> HarnessResult<ControllerLeaseRenewal> {
        let mut state = self.inner.lock().await;
        let token = format!("renewed-{}", now_millis());
        state.register_response.controller_token = Some(token.clone());
        Ok(ControllerLeaseRenewal {
            controller_token: token,
            expires_at_ms: now_millis() + 1_000,
        })
    }
}

// ----------------------------------------------------------------------------- //
// Tests
// ----------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use std::time::Duration;
    use tokio::time::sleep;

    fn sample_register_response() -> RegisterSessionResponse {
        RegisterSessionResponse {
            harness_id: "harness-1".into(),
            controller_token: Some("controller-1".into()),
            lease_ttl_ms: 30_000,
            state_cache_url: Some("redis://localhost:6379".into()),
            transport_hints: HashMap::new(),
        }
    }

    fn make_terminal_frame(line: &str) -> TerminalFrame {
        TerminalFrame {
            lines: vec![line.into()],
            styled_lines: None,
            styles: None,
            cols: None,
            rows: None,
            cursor: Some(CursorPosition {
                row: 0,
                col: line.len(),
            }),
        }
    }

    #[tokio::test]
    async fn terminal_diff_emitted_and_sequence_increments() {
        let transport = InMemoryTransport::with_response(sample_register_response());
        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "sess-1".into(),
                private_beach_id: "pb-1".into(),
                harness_type: HarnessType::TerminalShim,
                capabilities: vec!["terminal_diff_v1".into()],
                location_hint: Some("us-east-1".into()),
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport.clone(),
        );

        harness.register(None).await.unwrap();

        let diff1 = harness
            .push_terminal_frame(make_terminal_frame("hello"))
            .await
            .unwrap();
        assert_eq!(diff1.sequence, 1);

        let diff2 = harness
            .push_terminal_frame(make_terminal_frame("world"))
            .await
            .unwrap();
        assert_eq!(diff2.sequence, 2);

        let diffs = transport.diffs().await;
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].payload["type"], "terminal_full");
        assert_eq!(diffs[1].payload["lines"][0], "world");
    }

    #[tokio::test]
    async fn controller_preemption_flushes_pending_actions() {
        let transport = InMemoryTransport::with_response(sample_register_response());
        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "sess-ctrl".into(),
                private_beach_id: "pb-ctrl".into(),
                harness_type: HarnessType::TerminalShim,
                capabilities: vec!["terminal_diff_v1".into()],
                location_hint: None,
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport.clone(),
        );

        harness.register(None).await.unwrap();

        transport
            .push_actions(vec![
                ActionCommand {
                    id: "cmd-1".into(),
                    action_type: "terminal_write".into(),
                    payload: serde_json::json!({"bytes": "ping"}),
                    expires_at: None,
                },
                ActionCommand {
                    id: "cmd-2".into(),
                    action_type: "terminal_write".into(),
                    payload: serde_json::json!({"bytes": "pong"}),
                    expires_at: None,
                },
            ])
            .await;

        let cmds = harness.poll_actions().await.unwrap();
        assert_eq!(cmds.len(), 2);

        harness
            .handle_controller_notification(ControllerNotification {
                controller_token: Some("controller-2".into()),
                reason: Some("human takeover".into()),
            })
            .await
            .unwrap();

        let acks = transport.acks().await;
        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].len(), 2);
        assert!(acks[0]
            .iter()
            .all(|ack| matches!(ack.status, AckStatus::Preempted)));
    }

    #[tokio::test]
    async fn health_heartbeat_defaults_queue_depth() {
        let transport = InMemoryTransport::with_response(sample_register_response());
        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "sess-health".into(),
                private_beach_id: "pb-health".into(),
                harness_type: HarnessType::CabanaAdapter,
                capabilities: vec!["gui_frame_meta_v1".into()],
                location_hint: None,
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport.clone(),
        );

        harness.register(None).await.unwrap();

        harness
            .signal_health(HealthHeartbeat {
                queue_depth: usize::MAX,
                cpu_load: Some(0.42),
                memory_bytes: Some(1024),
                degraded: false,
                warnings: vec![],
            })
            .await
            .unwrap();

        let heartbeats = transport.health().await;
        assert_eq!(heartbeats.len(), 1);
        assert_eq!(heartbeats[0].queue_depth, 0);
        assert_eq!(heartbeats[0].cpu_load, Some(0.42));
    }

    #[tokio::test]
    async fn cabana_frame_payload_contains_metadata() {
        let transport = InMemoryTransport::with_response(sample_register_response());
        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "sess-gui".into(),
                private_beach_id: "pb-gui".into(),
                harness_type: HarnessType::CabanaAdapter,
                capabilities: vec!["gui_frame_meta_v1".into()],
                location_hint: Some("eu-west-1".into()),
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport.clone(),
        );

        harness.register(None).await.unwrap();

        let frame = CabanaFrame {
            fps: 59.9,
            cursor: Some((0.4, 0.6)),
            mouse_buttons: vec!["left".into()],
            windows: vec![WindowRegion {
                id: "win-1".into(),
                title: Some("Editor".into()),
                bounds: Bounds {
                    x: 0.0,
                    y: 0.0,
                    width: 800.0,
                    height: 600.0,
                },
            }],
        };

        let diff = harness.push_cabana_frame(frame.clone()).await.unwrap();
        assert_eq!(diff.sequence, 1);
        assert_eq!(diff.payload["type"], "cabana_frame");
        assert_eq!(diff.payload["windows"][0]["title"], "Editor");
    }

    #[tokio::test]
    async fn controller_runtime_renews_and_updates_token() {
        let transport = InMemoryTransport::with_response(sample_register_response());
        transport
            .set_pairings(vec![ControllerPairing {
                pairing_id: "controller-1:child-1".into(),
                controller_session_id: "controller-1".into(),
                child_session_id: "child-1".into(),
                prompt_template: Some("Prioritise shell".into()),
                update_cadence: ControllerUpdateCadence::Fast,
                transport_status: None,
                created_at_ms: None,
                updated_at_ms: None,
            }])
            .await;

        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "controller-1".into(),
                private_beach_id: "pb-ctrl".into(),
                harness_type: HarnessType::TerminalShim,
                capabilities: vec!["terminal_diff_v1".into()],
                location_hint: None,
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport.clone(),
        );

        let register = harness.register(None).await.unwrap();
        assert_eq!(register.controller_token.as_deref(), Some("controller-1"));

        let handle = harness.spawn_controller_runtime(1_500);

        sleep(Duration::from_millis(1_600)).await;

        let token = harness.controller_token().await;
        let token = token.expect("controller token present");
        assert!(token.starts_with("renewed-"));

        handle.abort();
        let _ = handle.await;
    }

    #[derive(Default)]
    struct HttpState {
        register_hits: usize,
        last_auth_header: Option<String>,
        diffs: Vec<StateDiff>,
        acks: Vec<Vec<ActionAck>>,
        health: Vec<HealthHeartbeat>,
    }

    #[tokio::test]
    async fn http_transport_flows_with_auth_header() {
        let shared_state = Arc::new(Mutex::new(HttpState::default()));
        let poll_actions = vec![ActionCommand {
            id: "cmd-http-1".into(),
            action_type: "terminal_write".into(),
            payload: serde_json::json!({"bytes": "hello"}),
            expires_at: None,
        }];
        let poll_actions_clone = poll_actions.clone();
        let app_state = shared_state.clone();

        let router = Router::new()
            .route(
                "/sessions/register",
                post({
                    move |headers: HeaderMap,
                          State(state): State<Arc<Mutex<HttpState>>>,
                          Json(req): Json<RegisterSessionRequest>| async move {
                        let mut guard = state.lock().await;
                        guard.register_hits += 1;
                        if let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) {
                            if let Ok(value) = auth.to_str() {
                                guard.last_auth_header = Some(value.to_string());
                            }
                        }
                        Json(RegisterSessionResponse {
                            harness_id: format!("mock-{}", req.session_id),
                            controller_token: Some("controller-http".into()),
                            lease_ttl_ms: 30_000,
                            state_cache_url: None,
                            transport_hints: HashMap::new(),
                        })
                    }
                }),
            )
            .route(
                "/sessions/:id/state",
                post({
                    move |State(state): State<Arc<Mutex<HttpState>>>,
                          Path(_id): Path<String>,
                          Json(diff): Json<StateDiff>| async move {
                        state.lock().await.diffs.push(diff);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/sessions/:id/actions/poll",
                get({
                    let mut returned = false;
                    move |_path: Path<String>| {
                        let actions = if !returned {
                            returned = true;
                            poll_actions_clone.clone()
                        } else {
                            Vec::new()
                        };
                        async move { Json(actions) }
                    }
                }),
            )
            .route(
                "/sessions/:id/actions/ack",
                post({
                    move |State(state): State<Arc<Mutex<HttpState>>>,
                          Path(_id): Path<String>,
                          Json(acks): Json<Vec<ActionAck>>| async move {
                        state.lock().await.acks.push(acks);
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/sessions/:id/health",
                post({
                    move |State(state): State<Arc<Mutex<HttpState>>>,
                          Path(_id): Path<String>,
                          Json(h): Json<HealthHeartbeat>| async move {
                        state.lock().await.health.push(h);
                        StatusCode::OK
                    }
                }),
            )
            .with_state(app_state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                panic!("server error: {err}");
            }
        });

        let base_url = format!("http://{}", addr);
        let transport =
            HttpTransport::new(base_url, StaticTokenProvider::new("test-token")).unwrap();

        let harness = SessionHarness::new(
            HarnessConfig {
                session_id: "sess-http".into(),
                private_beach_id: "pb-http".into(),
                harness_type: HarnessType::TerminalShim,
                capabilities: vec!["terminal_diff_v1".into()],
                location_hint: None,
                version: "0.1.0".into(),
                viewer_passcode: None,
            },
            transport,
        );

        let register_resp = harness.register(None).await.unwrap();
        assert_eq!(register_resp.harness_id, "mock-sess-http");

        harness
            .push_terminal_frame(make_terminal_frame("http test"))
            .await
            .unwrap();

        let cmds = harness.poll_actions().await.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, "cmd-http-1");

        harness
            .ack_action("cmd-http-1", AckStatus::Ok, None)
            .await
            .unwrap();

        harness
            .signal_health(HealthHeartbeat {
                queue_depth: usize::MAX,
                cpu_load: None,
                memory_bytes: None,
                degraded: false,
                warnings: vec!["all good".into()],
            })
            .await
            .unwrap();

        let state = app_state.lock().await;
        assert_eq!(state.register_hits, 1);
        assert_eq!(state.last_auth_header.as_deref(), Some("Bearer test-token"));
        assert_eq!(state.diffs.len(), 1);
        assert_eq!(state.acks.len(), 1);
        assert_eq!(state.acks[0][0].id, "cmd-http-1");
        assert_eq!(state.health.len(), 1);
        assert_eq!(state.health[0].warnings[0], "all good");
    }
}

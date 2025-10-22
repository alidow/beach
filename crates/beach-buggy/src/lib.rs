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
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::SystemTime,
};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{trace, warn};

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

/// Representation of a terminal frame we can diff.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFrame {
    pub lines: Vec<String>,
    pub cursor: Option<CursorPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

fn build_terminal_payload(frame: &TerminalFrame) -> serde_json::Value {
    serde_json::json!({
        "type": "terminal_full",
        "lines": frame.lines,
        "cursor": frame.cursor.map(|c| serde_json::json!({"row": c.row, "col": c.col}))
    })
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
        })
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
        resp.json::<RegisterSessionResponse>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode register response: {e}")))
    }

    async fn send_state(&self, session_id: &str, diff: StateDiff) -> HarnessResult<()> {
        let url = self.url(&format!("sessions/{session_id}/state"))?;
        self.request_with_token(self.client.post(url).json(&diff))
            .await?;
        Ok(())
    }

    async fn receive_actions(&self, session_id: &str) -> HarnessResult<Vec<ActionCommand>> {
        let url = self.url(&format!("sessions/{session_id}/actions/poll"))?;
        let resp = self.request_with_token(self.client.get(url)).await?;
        resp.json::<Vec<ActionCommand>>()
            .await
            .map_err(|e| HarnessError::Transport(format!("decode actions: {e}")))
    }

    async fn ack_actions(&self, session_id: &str, acks: Vec<ActionAck>) -> HarnessResult<()> {
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

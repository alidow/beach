//! Control-plane state for the Beach Manager service.
//!
//! The **manager** refers to this Rust control plane (`apps/beach-manager`). A **controller**
//! is any Beach session whose Beach Buggy harness currently holds a controller lease via the
//! manager APIs. Controllers drive other sessions; non-controller harnesses simply stream
//! state into the manager until a lease is granted.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::auth::{AuthConfig, AuthContext};
use crate::fastpath::{FastPathRegistry, FastPathSession, send_actions_over_fast_path};
use crate::metrics;
use beach_buggy::{
    AckStatus, ActionAck, ActionCommand, CursorPosition, HarnessType, HealthHeartbeat,
    RegisterSessionRequest, RegisterSessionResponse, StateDiff, TerminalFrame,
};
use beach_client_core::protocol::{CursorFrame, Update as WireUpdate};
use beach_client_core::{
    CliError, ClientFrame as WireClientFrame, HostFrame as WireHostFrame, NegotiatedSingle,
    NegotiatedTransport, PackedCell, Payload, SessionConfig, SessionError, SessionManager, Style,
    StyleId, TerminalGrid, TransportError, decode_host_frame_binary, encode_client_frame_binary,
    negotiate_transport,
};
use chrono::{DateTime, Duration, Utc};
use prometheus::IntGauge;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Row, types::Json};
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use uuid::Uuid;

const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
const REDIS_ACTION_STREAM_MAXLEN: usize = 2_048;
const REDIS_TTL_SECONDS: usize = 120;
const REDIS_ACTION_GROUP: &str = "controllers";
const REDIS_ACTION_CONSUMER_PREFIX: &str = "poller";
const VIEWER_KEEPALIVE_INTERVAL: StdDuration = StdDuration::from_secs(20);

#[derive(Clone)]
pub struct AppState {
    backend: Backend,
    fallback: Arc<InnerState>,
    redis: Option<Arc<redis::Client>>,
    auth: Arc<AuthContext>,
    events: Arc<RwLock<HashMap<String, broadcast::Sender<StreamEvent>>>>,
    fast_paths: FastPathRegistry,
    viewer_workers: Arc<RwLock<HashMap<String, ViewerWorker>>>,
    viewer_tokens: Option<ViewerTokenClient>,
    http: reqwest::Client,
    road_base_url: String,
    public_manager_url: String,
}

#[derive(Clone)]
enum Backend {
    Memory,
    Postgres(PgPool),
}

struct InnerState {
    sessions: RwLock<HashMap<String, SessionRecord>>,
}

#[derive(Debug, Clone)]
struct SessionRecord {
    session_id: String,
    private_beach_id: String,
    harness_type: HarnessType,
    capabilities: Vec<String>,
    location_hint: Option<String>,
    metadata: Option<serde_json::Value>,
    version: String,
    harness_id: String,
    controller_token: Option<String>,
    viewer_passcode: Option<String>,
    lease_ttl_ms: u64,
    transport_hints: HashMap<String, serde_json::Value>,
    state_cache_url: Option<String>,
    pending_actions: VecDeque<ActionCommand>,
    controller_events: Vec<ControllerEvent>,
    last_health: Option<HealthHeartbeat>,
    last_state: Option<StateDiff>,
}

struct ViewerWorker {
    handle: JoinHandle<()>,
}

#[derive(Clone)]
struct ViewerTokenClient {
    base_url: Arc<String>,
    bearer: Arc<String>,
}

impl ViewerTokenClient {
    fn new(base_url: String, bearer: String) -> Self {
        let trimmed = base_url.trim_end_matches('/').to_string();
        Self {
            base_url: Arc::new(trimmed),
            bearer: Arc::new(bearer),
        }
    }

    async fn issue(
        &self,
        http: &reqwest::Client,
        session_id: &str,
        private_beach_id: &str,
        join_code: &str,
    ) -> Result<ViewerTokenIssued, ViewerTokenError> {
        let url = format!("{}/viewer/credentials", self.base_url);
        let response = http
            .post(url)
            .bearer_auth(self.bearer.as_ref())
            .json(&serde_json::json!({
                "sessionId": session_id,
                "joinCode": join_code,
                "privateBeachId": private_beach_id,
            }))
            .send()
            .await
            .map_err(ViewerTokenError::Http)?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(ViewerTokenError::Unauthorized);
        }
        if response.status() == StatusCode::SERVICE_UNAVAILABLE {
            return Err(ViewerTokenError::Unavailable);
        }
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(ViewerTokenError::Upstream(format!(
                "viewer token request failed: status {} body {}",
                status, detail
            )));
        }

        let payload: ViewerTokenGatewayResponse =
            response.json().await.map_err(ViewerTokenError::Http)?;

        Ok(ViewerTokenIssued {
            token: payload.token,
            expires_at_ms: payload.expires_at,
        })
    }
}

#[derive(Deserialize)]
struct ViewerTokenGatewayResponse {
    token: String,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    expires_in: Option<u64>,
}

struct ViewerTokenIssued {
    token: String,
    expires_at_ms: Option<i64>,
}

#[derive(Debug)]
pub enum ViewerTokenError {
    Unauthorized,
    Unavailable,
    Http(reqwest::Error),
    Upstream(String),
}
#[derive(Clone, Copy, Debug)]
struct ViewerCursor {
    abs_row: usize,
    col: usize,
}

struct ManagerViewerState {
    grid: TerminalGrid,
    cursor: Option<ViewerCursor>,
    diff_seq: AtomicU64,
}

impl ManagerViewerState {
    fn new() -> Self {
        Self {
            grid: TerminalGrid::with_history_limit(1, 1, 1024),
            cursor: None,
            diff_seq: AtomicU64::new(0),
        }
    }

    fn reset_grid(
        &mut self,
        cols: u32,
        history_rows: u32,
        base_row: u64,
        viewport_rows: Option<u32>,
    ) {
        let viewport = viewport_rows.unwrap_or(history_rows).max(1) as usize;
        let cols = cols.max(1) as usize;
        let history_limit = history_rows.max(viewport_rows.unwrap_or(history_rows)) as usize;
        self.grid = TerminalGrid::with_history_limit(viewport, cols, history_limit.max(viewport));
        self.grid.set_viewport_size(viewport, cols);
        self.grid.set_row_offset(base_row);
        self.cursor = None;
    }

    fn apply_updates(&self, updates: &[WireUpdate]) {
        for update in updates {
            self.apply_update(update);
        }
    }

    fn apply_update(&self, update: &WireUpdate) {
        match update {
            WireUpdate::Cell {
                row,
                col,
                seq,
                cell,
            } => {
                let _ = self.grid.write_packed_cell_if_newer(
                    *row as usize,
                    *col as usize,
                    *seq,
                    PackedCell::from(*cell),
                );
            }
            WireUpdate::Row { row, seq, cells } => {
                let row_idx = *row as usize;
                for (offset, cell) in cells.iter().enumerate() {
                    let _ = self.grid.write_packed_cell_if_newer(
                        row_idx,
                        offset,
                        *seq,
                        PackedCell::from(*cell),
                    );
                }
            }
            WireUpdate::RowSegment {
                row,
                start_col,
                seq,
                cells,
            } => {
                let row_idx = *row as usize;
                let base_col = *start_col as usize;
                for (offset, cell) in cells.iter().enumerate() {
                    let _ = self.grid.write_packed_cell_if_newer(
                        row_idx,
                        base_col + offset,
                        *seq,
                        PackedCell::from(*cell),
                    );
                }
            }
            WireUpdate::Rect {
                rows,
                cols,
                seq,
                cell,
            } => {
                let row0 = rows[0] as usize;
                let row1 = rows[1] as usize;
                let col0 = cols[0] as usize;
                let col1 = cols[1] as usize;
                let _ = self.grid.fill_rect_with_cell_if_newer(
                    row0,
                    col0,
                    row1,
                    col1,
                    *seq,
                    PackedCell::from(*cell),
                );
            }
            WireUpdate::Trim { count, .. } => {
                if *count > 0 {
                    let base = self.grid.row_offset();
                    let new_base = base.saturating_add(*count as u64);
                    self.grid.set_row_offset(new_base);
                }
            }
            WireUpdate::Style {
                id, fg, bg, attrs, ..
            } => {
                let style = Style {
                    fg: *fg,
                    bg: *bg,
                    attrs: *attrs,
                };
                if !self.grid.style_table.set(StyleId(*id), style) {
                    debug!(
                        target = "private_beach",
                        style_id = *id,
                        "manager viewer received style update for missing style id"
                    );
                }
            }
        }
    }

    fn update_cursor(&mut self, frame: &CursorFrame) {
        self.cursor = Some(ViewerCursor {
            abs_row: frame.row as usize,
            col: frame.col as usize,
        });
    }

    fn handle_host_frame(&mut self, frame: &WireHostFrame) -> Option<StateDiff> {
        match frame {
            WireHostFrame::Hello { .. } => {
                self.cursor = None;
                self.diff_seq.store(0, Ordering::SeqCst);
                None
            }
            WireHostFrame::Grid {
                cols,
                history_rows,
                base_row,
                viewport_rows,
            } => {
                self.reset_grid(*cols, *history_rows, *base_row, *viewport_rows);
                None
            }
            WireHostFrame::Snapshot {
                updates, cursor, ..
            }
            | WireHostFrame::Delta {
                updates, cursor, ..
            }
            | WireHostFrame::HistoryBackfill {
                updates, cursor, ..
            } => {
                self.apply_updates(updates);
                if let Some(cursor_frame) = cursor {
                    self.update_cursor(cursor_frame);
                }
                Some(self.build_diff())
            }
            WireHostFrame::Cursor { cursor, .. } => {
                self.update_cursor(cursor);
                Some(self.build_diff())
            }
            WireHostFrame::SnapshotComplete { .. }
            | WireHostFrame::Heartbeat { .. }
            | WireHostFrame::InputAck { .. }
            | WireHostFrame::Shutdown => None,
        }
    }

    fn build_diff(&self) -> StateDiff {
        let frame = capture_terminal_frame_simple(&self.grid, self.cursor.as_ref());
        let sequence = self.diff_seq.fetch_add(1, Ordering::SeqCst) + 1;
        StateDiff {
            sequence,
            emitted_at: SystemTime::now(),
            payload: build_terminal_payload(&frame),
        }
    }
}

fn capture_terminal_frame_simple(
    grid: &TerminalGrid,
    cursor: Option<&ViewerCursor>,
) -> TerminalFrame {
    let (viewport_rows, viewport_cols) = grid.viewport_size();
    let total_rows = grid.rows();
    let rows = viewport_rows.min(total_rows);
    let style_table = grid.style_table.clone();
    let mut lines = Vec::with_capacity(rows);

    for row in 0..rows {
        let mut line = String::with_capacity(viewport_cols);
        for col in 0..viewport_cols {
            let ch = grid
                .get_cell_relaxed(row, col)
                .map(|snapshot| snapshot.unpack(style_table.as_ref()).char)
                .unwrap_or(' ');
            line.push(ch);
        }
        while line.ends_with(' ') {
            line.pop();
        }
        lines.push(line);
    }

    let cursor = cursor.map(|cursor| CursorPosition {
        row: cursor.abs_row,
        col: cursor.col,
    });

    TerminalFrame { lines, cursor }
}

fn build_terminal_payload(frame: &TerminalFrame) -> serde_json::Value {
    serde_json::json!({
        "type": "terminal_full",
        "lines": frame.lines,
        "cursor": frame.cursor.map(|cursor| serde_json::json!({
            "row": cursor.row,
            "col": cursor.col,
        })),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("session not found")]
    SessionNotFound,
    #[error("controller token mismatch")]
    ControllerMismatch,
    #[error("private beach not found")]
    PrivateBeachNotFound,
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("external service error: {0}")]
    External(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub private_beach_id: String,
    pub harness_type: HarnessType,
    pub capabilities: Vec<String>,
    pub location_hint: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub version: String,
    pub harness_id: String,
    pub controller_token: Option<String>,
    pub controller_expires_at_ms: Option<i64>,
    pub pending_actions: usize,
    pub pending_unacked: usize,
    pub last_health: Option<HealthHeartbeat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControllerEvent {
    pub id: String,
    pub event_type: ControllerEventType,
    pub controller_token: Option<String>,
    pub timestamp_ms: i64,
    pub reason: Option<String>,
    pub controller_account_id: Option<String>,
    pub issued_by_account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerEventType {
    Registered,
    LeaseAcquired,
    LeaseReleased,
    ActionsQueued,
    ActionsAcked,
    HealthReported,
    StateUpdated,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum StreamEvent {
    ControllerEvent(ControllerEvent),
    State(StateDiff),
    Health(HealthHeartbeat),
}

impl StreamEvent {
    pub fn as_named_json(&self) -> (&'static str, Option<String>) {
        match self {
            StreamEvent::ControllerEvent(ev) => {
                ("controller_event", serde_json::to_string(ev).ok())
            }
            StreamEvent::State(diff) => ("state", serde_json::to_string(diff).ok()),
            StreamEvent::Health(hb) => ("health", serde_json::to_string(hb).ok()),
        }
    }
}

/// Payload returned to a controller session when the manager grants (or renews) a lease.
#[derive(Debug, Clone, Serialize)]
pub struct ControllerLeaseResponse {
    pub controller_token: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentOnboardResponse {
    pub agent_token: String,
    pub prompt_pack: serde_json::Value,
    pub mcp_bridges: Vec<McpBridge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpBridge {
    pub id: String,
    pub name: String,
    pub description: String,
    pub endpoint: Option<String>,
}

#[derive(Debug, FromRow)]
struct DbSessionIdentifiers {
    session_id: Uuid,
    private_beach_id: Uuid,
}

#[derive(Debug, FromRow)]
struct SessionRow {
    origin_session_id: Uuid,
    private_beach_id: Uuid,
    harness_id: Option<Uuid>,
    harness_type: Option<HarnessTypeDb>,
    capabilities: Option<Json<serde_json::Value>>,
    metadata: Option<Json<serde_json::Value>>,
    location_hint: Option<String>,
    last_health: Option<Json<serde_json::Value>>,
    controller_token: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct ControllerEventRow {
    id: Uuid,
    event_type: String,
    controller_token: Option<Uuid>,
    reason: Option<String>,
    occurred_at: DateTime<Utc>,
    controller_account_id: Option<Uuid>,
    issued_by_account_id: Option<Uuid>,
}

impl AppState {
    pub fn new() -> Self {
        let auth_config = AuthConfig {
            bypass: true,
            ..AuthConfig::default()
        };
        Self {
            backend: Backend::Memory,
            fallback: Arc::new(InnerState::new()),
            redis: None,
            auth: Arc::new(AuthContext::new(auth_config)),
            events: Arc::new(RwLock::new(HashMap::new())),
            fast_paths: FastPathRegistry::new(),
            viewer_workers: Arc::new(RwLock::new(HashMap::new())),
            viewer_tokens: None,
            http: reqwest::Client::new(),
            road_base_url: std::env::var("BEACH_ROAD_URL")
                .unwrap_or_else(|_| "https://api.beach.sh".into()),
            public_manager_url: std::env::var("PUBLIC_MANAGER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
        }
    }

    pub fn with_db(pool: PgPool) -> Self {
        let auth_config = AuthConfig {
            bypass: true,
            ..AuthConfig::default()
        };
        Self {
            backend: Backend::Postgres(pool),
            fallback: Arc::new(InnerState::new()),
            redis: None,
            auth: Arc::new(AuthContext::new(auth_config)),
            events: Arc::new(RwLock::new(HashMap::new())),
            fast_paths: FastPathRegistry::new(),
            viewer_workers: Arc::new(RwLock::new(HashMap::new())),
            viewer_tokens: None,
            http: reqwest::Client::new(),
            road_base_url: std::env::var("BEACH_ROAD_URL")
                .unwrap_or_else(|_| "https://api.beach.sh".into()),
            public_manager_url: std::env::var("PUBLIC_MANAGER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
        }
    }

    pub fn with_redis(mut self, client: redis::Client) -> Self {
        self.redis = Some(Arc::new(client));
        self
    }

    pub fn with_auth(mut self, auth: AuthContext) -> Self {
        self.auth = Arc::new(auth);
        self
    }

    pub fn with_integrations(
        mut self,
        road_base_url: Option<String>,
        public_manager_url: Option<String>,
    ) -> Self {
        if let Some(url) = road_base_url {
            self.road_base_url = url;
        }
        if let Some(url) = public_manager_url {
            self.public_manager_url = url;
        }
        self
    }

    pub fn with_viewer_tokens(
        mut self,
        gate_url: Option<String>,
        service_token: Option<String>,
    ) -> Self {
        if let (Some(url), Some(token)) = (gate_url, service_token) {
            self.viewer_tokens = Some(ViewerTokenClient::new(url, token));
        }
        self
    }

    pub async fn viewer_token(
        &self,
        session_id: &str,
        private_beach_id: &str,
        join_code: &str,
    ) -> Result<ViewerTokenIssued, ViewerTokenError> {
        match &self.viewer_tokens {
            Some(client) => {
                client
                    .issue(&self.http, session_id, private_beach_id, join_code)
                    .await
            }
            None => Err(ViewerTokenError::Unavailable),
        }
    }

    #[allow(dead_code)]
    pub fn db_pool(&self) -> Option<&PgPool> {
        match &self.backend {
            Backend::Postgres(pool) => Some(pool),
            Backend::Memory => None,
        }
    }

    pub fn auth_context(&self) -> Arc<AuthContext> {
        self.auth.clone()
    }

    pub async fn attach_fast_path(&self, session_id: String, fps: FastPathSession) {
        let arc = Arc::new(fps);
        self.fast_paths.insert(session_id, arc).await;
    }

    pub async fn fast_path_for(&self, session_id: &str) -> Option<Arc<FastPathSession>> {
        self.fast_paths.get(session_id).await
    }

    pub async fn subscribe_session(&self, session_id: &str) -> broadcast::Receiver<StreamEvent> {
        let mut map = self.events.write().await;
        let tx = map
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(128).0)
            .clone();
        info!(session_id = %session_id, "session stream subscribed");
        tx.subscribe()
    }

    async fn publish(&self, session_id: &str, event: StreamEvent) {
        let tx_opt = { self.events.read().await.get(session_id).cloned() };
        if let Some(tx) = tx_opt {
            let event_kind = match &event {
                StreamEvent::ControllerEvent(_) => "controller_event",
                StreamEvent::State(_) => "state",
                StreamEvent::Health(_) => "health",
            };
            if tx.send(event).is_err() {
                info!(
                    session_id = %session_id,
                    event_kind,
                    "no subscribers to receive stream event"
                );
            }
        }
    }

    pub async fn register_session(
        &self,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        match &self.backend {
            Backend::Memory => self.register_session_memory(req).await,
            Backend::Postgres(pool) => self.register_session_postgres(pool, req).await,
        }
    }

    // Session onboarding helpers
    pub async fn attach_by_code(
        &self,
        private_beach_id: &str,
        origin_session_id: &str,
        code: &str,
        requester: Option<Uuid>,
    ) -> Result<SessionSummary, StateError> {
        // Verify with Beach Road
        let verified = self
            .verify_code_with_road(origin_session_id, code)
            .await
            .unwrap_or(false);
        if !verified {
            return Err(StateError::InvalidIdentifier("invalid_code".into()));
        }
        // Create mapping if not exists
        match &self.backend {
            Backend::Memory => {
                let mut sessions = self.fallback.sessions.write().await;
                let rec = sessions
                    .entry(origin_session_id.to_string())
                    .or_insert_with(|| {
                        SessionRecord::new(
                            origin_session_id,
                            private_beach_id,
                            &HarnessType::Custom,
                        )
                    });
                rec.viewer_passcode = Some(code.to_string());
                rec.append_event(
                    ControllerEventType::Registered,
                    Some("attach_by_code".into()),
                );
                if let Err(err) = self.spawn_viewer_worker(origin_session_id).await {
                    warn!(
                        target = "private_beach",
                        session_id = %origin_session_id,
                        error = %err,
                        "failed to start viewer worker after attach_by_code (memory backend)"
                    );
                }
                Ok(SessionSummary::from_record(rec))
            }
            Backend::Postgres(pool) => {
                let beach_uuid = parse_uuid(private_beach_id, "private_beach_id")?;
                let origin_uuid = parse_uuid(origin_session_id, "session_id")?;
                info!(
                    private_beach_id = %beach_uuid,
                    origin_session_id = %origin_uuid,
                    requester = ?requester,
                    "attaching session via code"
                );
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &beach_uuid).await?;
                // Ensure the target private beach exists to avoid FK violations
                let exists: Option<(i32,)> =
                    sqlx::query_as(r#"SELECT 1 FROM private_beach WHERE id = $1"#)
                        .bind(beach_uuid)
                        .fetch_optional(tx.as_mut())
                        .await?;
                if exists.is_none() {
                    return Err(StateError::PrivateBeachNotFound);
                }
                // Insert session row if not exists, leave harness fields null
                let _ = sqlx::query(
                    r#"
                    INSERT INTO session (private_beach_id, origin_session_id, kind, created_by_account_id, attach_method)
                    VALUES ($1, $2, 'terminal', $3, 'code')
                    ON CONFLICT (private_beach_id, origin_session_id) DO NOTHING
                    "#,
                )
                .bind(beach_uuid)
                .bind(origin_uuid)
                .bind(requester)
                .execute(tx.as_mut())
                .await?;

                // Emit a controller_event of type registered to reflect attach
                // Look up db session id
                let ids = sqlx::query_as::<_, DbSessionIdentifiers>(
                    r#"SELECT id AS session_id, private_beach_id FROM session WHERE private_beach_id = $1 AND origin_session_id = $2"#,
                )
                .bind(beach_uuid)
                .bind(origin_uuid)
                .fetch_one(tx.as_mut())
                .await?;
                self.insert_controller_event(
                    &mut tx,
                    ids.session_id,
                    "registered",
                    None,
                    requester,
                    requester,
                    Some("attach_by_code".into()),
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO session_runtime (session_id, viewer_passcode)
                    VALUES ($1, $2)
                    ON CONFLICT (session_id)
                    DO UPDATE SET viewer_passcode = EXCLUDED.viewer_passcode
                    "#,
                )
                .bind(ids.session_id)
                .bind(code)
                .execute(tx.as_mut())
                .await?;
                tx.commit().await?;

                {
                    let mut sessions = self.fallback.sessions.write().await;
                    let rec = sessions
                        .entry(origin_session_id.to_string())
                        .or_insert_with(|| {
                            SessionRecord::new(
                                origin_session_id,
                                private_beach_id,
                                &HarnessType::Custom,
                            )
                        });
                    rec.viewer_passcode = Some(code.to_string());
                }

                if let Err(err) = self.spawn_viewer_worker(origin_session_id).await {
                    warn!(
                        target = "private_beach",
                        session_id = %origin_session_id,
                        error = %err,
                        "failed to start viewer worker after attach_by_code"
                    );
                }

                // Return summary (best-effort from DB fields)
                let mut list = self.list_sessions(private_beach_id).await?;
                if let Some(found) = list.iter().find(|s| s.session_id == origin_session_id) {
                    return Ok(found.clone());
                }
                Err(StateError::SessionNotFound)
            }
        }
    }

    pub async fn attach_owned(
        &self,
        private_beach_id: &str,
        origin_ids: Vec<String>,
        requester: Option<Uuid>,
    ) -> Result<(usize, usize), StateError> {
        let mut attached = 0usize;
        let mut duplicates = 0usize;
        match &self.backend {
            Backend::Memory => {
                let mut sessions = self.fallback.sessions.write().await;
                for id in origin_ids {
                    let existed = sessions.contains_key(&id);
                    let entry = sessions.entry(id.clone()).or_insert_with(|| {
                        SessionRecord::new(&id, private_beach_id, &HarnessType::Custom)
                    });
                    if existed {
                        duplicates += 1;
                    } else {
                        attached += 1;
                        entry.append_event(
                            ControllerEventType::Registered,
                            Some("attach_owned".into()),
                        );
                    }
                }
                Ok((attached, duplicates))
            }
            Backend::Postgres(pool) => {
                let beach_uuid = parse_uuid(private_beach_id, "private_beach_id")?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &beach_uuid).await?;
                // Ensure the target private beach exists to avoid FK violations
                let exists: Option<(i32,)> =
                    sqlx::query_as(r#"SELECT 1 FROM private_beach WHERE id = $1"#)
                        .bind(beach_uuid)
                        .fetch_optional(tx.as_mut())
                        .await?;
                if exists.is_none() {
                    return Err(StateError::PrivateBeachNotFound);
                }
                let mut ids_to_nudge: Vec<String> = Vec::new();
                for id in origin_ids {
                    if let Ok(origin_uuid) = Uuid::parse_str(&id) {
                        let result = sqlx::query(
                    r#"
                    INSERT INTO session (private_beach_id, origin_session_id, kind, created_by_account_id, attach_method)
                    VALUES ($1, $2, 'terminal', $3, 'owned')
                    ON CONFLICT (private_beach_id, origin_session_id) DO NOTHING
                    "#,
                )
                .bind(beach_uuid)
                .bind(origin_uuid)
                .bind(requester)
                .execute(tx.as_mut())
                .await?;
                        if result.rows_affected() == 0 {
                            duplicates += 1;
                        } else {
                            attached += 1;
                            ids_to_nudge.push(id.clone());
                        }
                    }
                }
                tx.commit().await?;
                Ok((attached, duplicates))
            }
        }
    }

    async fn verify_code_with_road(&self, origin_session_id: &str, code: &str) -> Result<bool, ()> {
        let url = format!(
            "{}/sessions/{}/verify-code",
            self.road_base_url.trim_end_matches('/'),
            origin_session_id
        );
        let body = serde_json::json!({ "code": code });
        let resp = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| ())?;
        if !resp.status().is_success() {
            return Ok(false);
        }
        let v: serde_json::Value = resp.json().await.map_err(|_| ())?;
        Ok(v.get("verified").and_then(|b| b.as_bool()).unwrap_or(false))
    }

    pub async fn join_session_via_road(
        &self,
        session_id: &str,
        passphrase: Option<String>,
        mcp: bool,
    ) -> Result<(StatusCode, JoinSessionResponsePayload), JoinForwardError> {
        let url = format!(
            "{}/sessions/{}/join",
            self.road_base_url.trim_end_matches('/'),
            session_id
        );
        let body = serde_json::json!({
            "passphrase": passphrase,
            "mcp": mcp,
        });
        let response = self.http.post(url).json(&body).send().await?;
        let status = response.status();
        let text = response.text().await?;
        let payload: JoinSessionResponsePayload = serde_json::from_str(&text)?;
        Ok((status, payload))
    }

    pub async fn update_session_metadata(
        &self,
        session_id: &str,
        metadata: Option<serde_json::Value>,
        location_hint: Option<String>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                let mut sessions = self.fallback.sessions.write().await;
                let record = sessions
                    .get_mut(session_id)
                    .ok_or(StateError::SessionNotFound)?;
                if let Some(meta) = metadata {
                    record.metadata = Some(meta);
                }
                if let Some(loc) = location_hint {
                    record.location_hint = Some(loc);
                }
                Ok(())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                // Discover identifiers and set RLS context inside a transaction.
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let (meta_json, loc) = (
                    metadata.unwrap_or_else(|| serde_json::json!({})),
                    location_hint,
                );
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                let result = sqlx::query(
                    r#"
                    UPDATE session
                    SET metadata = $2,
                        location_hint = COALESCE($3, location_hint),
                        last_seen_at = NOW()
                    WHERE origin_session_id = $1
                    "#,
                )
                .bind(session_uuid)
                .bind(Json(meta_json))
                .bind(loc)
                .execute(tx.as_mut())
                .await?;

                if result.rows_affected() == 0 {
                    return Err(StateError::SessionNotFound);
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub async fn list_sessions(
        &self,
        private_beach_id: &str,
    ) -> Result<Vec<SessionSummary>, StateError> {
        match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                let summaries = sessions
                    .values()
                    .filter(|record| record.private_beach_id == private_beach_id)
                    .map(SessionSummary::from_record)
                    .collect();
                Ok(summaries)
            }
            Backend::Postgres(pool) => {
                let beach_uuid = parse_uuid(private_beach_id, "private_beach_id")?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &beach_uuid).await?;
                let rows: Vec<SessionRow> = sqlx::query_as(
                    r#"
                    SELECT
                        s.origin_session_id,
                        s.private_beach_id,
                        s.harness_id,
                        s.harness_type,
                        s.capabilities,
                        s.metadata,
                        s.location_hint,
                        sr.last_health,
                        cl.id AS controller_token,
                        cl.expires_at,
                        cl.revoked_at
                    FROM session s
                    LEFT JOIN session_runtime sr ON sr.session_id = s.id
                    LEFT JOIN controller_lease cl ON cl.session_id = s.id
                    WHERE s.private_beach_id = $1
                    ORDER BY s.created_at ASC
                    "#,
                )
                .bind(beach_uuid)
                .fetch_all(tx.as_mut())
                .await?;

                let mut summaries = Vec::with_capacity(rows.len());
                for row in rows {
                    let harness = row
                        .harness_type
                        .map(HarnessType::from)
                        .unwrap_or(HarnessType::Custom);
                    let capabilities = row
                        .capabilities
                        .map(|Json(value)| json_array_to_strings(&value))
                        .unwrap_or_default();
                    let metadata = row.metadata.map(|Json(value)| value);
                    let controller_token = row
                        .controller_token
                        .filter(|_| is_active_lease(row.expires_at, row.revoked_at))
                        .map(|token| token.to_string());
                    let controller_expires_at_ms =
                        if is_active_lease(row.expires_at, row.revoked_at) {
                            row.expires_at.map(|t| t.timestamp_millis())
                        } else {
                            None
                        };
                    let last_health = row
                        .last_health
                        .and_then(|Json(value)| serde_json::from_value(value).ok());
                    let pending_actions = self
                        .pending_actions_count(
                            &row.private_beach_id.to_string(),
                            &row.origin_session_id.to_string(),
                        )
                        .await?;
                    let pending_unacked = self
                        .pending_actions_pending_count(
                            &row.private_beach_id.to_string(),
                            &row.origin_session_id.to_string(),
                        )
                        .await?;

                    summaries.push(SessionSummary {
                        session_id: row.origin_session_id.to_string(),
                        private_beach_id: row.private_beach_id.to_string(),
                        harness_type: harness,
                        capabilities,
                        location_hint: row.location_hint.clone(),
                        metadata,
                        version: "unknown".into(),
                        harness_id: row.harness_id.unwrap_or_else(Uuid::new_v4).to_string(),
                        controller_token,
                        controller_expires_at_ms,
                        pending_actions,
                        pending_unacked,
                        last_health,
                    });
                }
                tx.commit().await?;
                Ok(summaries)
            }
        }
    }

    pub async fn acquire_controller(
        &self,
        session_id: &str,
        ttl_override: Option<u64>,
        reason: Option<String>,
        requester: Option<Uuid>,
    ) -> Result<ControllerLeaseResponse, StateError> {
        match &self.backend {
            Backend::Memory => {
                self.acquire_controller_memory(session_id, ttl_override, reason.clone())
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let ttl = ttl_override.unwrap_or(DEFAULT_LEASE_TTL_MS).max(1_000);
                let expires_at = Utc::now() + Duration::milliseconds(ttl as i64);
                let controller_token = Uuid::new_v4();
                let requester_uuid = requester;

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO controller_lease (
                        id, session_id, controller_account_id, issued_by_account_id,
                        reason, issued_at, expires_at, revoked_at
                    )
                    VALUES ($1, $2, $3, $3, $4, NOW(), $5, NULL)
                    ON CONFLICT (session_id)
                    DO UPDATE SET
                        id = EXCLUDED.id,
                        controller_account_id = EXCLUDED.controller_account_id,
                        issued_by_account_id = EXCLUDED.issued_by_account_id,
                        reason = EXCLUDED.reason,
                        issued_at = NOW(),
                        expires_at = EXCLUDED.expires_at,
                        revoked_at = NULL
                    "#,
                )
                .bind(controller_token)
                .bind(identifiers.session_id)
                .bind(requester_uuid)
                .bind(reason.clone())
                .bind(expires_at)
                .execute(tx.as_mut())
                .await?;

                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_acquired",
                    Some(controller_token),
                    requester_uuid,
                    requester_uuid,
                    reason.clone(),
                )
                .await?;

                tx.commit().await?;

                self.fallback
                    .acknowledge_controller(session_id, controller_token.to_string(), ttl)
                    .await;

                // Emit a controller event on the SSE channel
                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::LeaseAcquired,
                        controller_token: Some(controller_token.to_string()),
                        timestamp_ms: now_ms(),
                        reason,
                        controller_account_id: requester_uuid.map(|u| u.to_string()),
                        issued_by_account_id: requester_uuid.map(|u| u.to_string()),
                    }),
                )
                .await;

                Ok(ControllerLeaseResponse {
                    controller_token: controller_token.to_string(),
                    expires_at_ms: expires_at.timestamp_millis(),
                })
            }
        }
    }

    pub async fn release_controller(
        &self,
        session_id: &str,
        controller_token: &str,
        actor_account_id: Option<Uuid>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                self.release_controller_memory(session_id, controller_token)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let token_uuid = parse_uuid(controller_token, "controller_token")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE controller_lease
                    SET revoked_at = NOW(), expires_at = NOW()
                    WHERE session_id = $1 AND id = $2
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(token_uuid)
                .execute(tx.as_mut())
                .await?;

                if updated.rows_affected() == 0 {
                    return Err(StateError::ControllerMismatch);
                }

                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_released",
                    Some(token_uuid),
                    None,
                    actor_account_id,
                    None,
                )
                .await?;

                tx.commit().await?;
                self.fallback.clear_controller(session_id).await;

                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::LeaseReleased,
                        controller_token: Some(token_uuid.to_string()),
                        timestamp_ms: now_ms(),
                        reason: None,
                        controller_account_id: None,
                        issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                    }),
                )
                .await;
                Ok(())
            }
        }
    }

    pub async fn queue_actions(
        &self,
        session_id: &str,
        controller_token: &str,
        actions: Vec<ActionCommand>,
        actor_account_id: Option<Uuid>,
    ) -> Result<(), StateError> {
        if actions.is_empty() {
            return Ok(());
        }

        match &self.backend {
            Backend::Memory => {
                self.queue_actions_memory(session_id, controller_token, actions)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let token_uuid = parse_uuid(controller_token, "controller_token")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let lease = self
                    .fetch_active_lease(pool, identifiers.session_id)
                    .await?;

                if lease.id != token_uuid {
                    return Err(StateError::ControllerMismatch);
                }

                let now = Utc::now();
                if lease
                    .expires_at
                    .map(|expires_at| expires_at < now)
                    .unwrap_or(true)
                {
                    return Err(StateError::ControllerMismatch);
                }

                // Try fast-path first
                if send_actions_over_fast_path(
                    &self.fast_paths,
                    &session_uuid.to_string(),
                    &actions,
                )
                .await
                .unwrap_or(false)
                {
                    let mut tx = pool.begin().await?;
                    self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                        .await?;
                    self.insert_controller_event(
                        &mut tx,
                        identifiers.session_id,
                        "actions_queued",
                        Some(lease.id),
                        lease.controller_account_id,
                        actor_account_id,
                        None,
                    )
                    .await?;
                    tx.commit().await?;

                    self.publish(
                        session_id,
                        StreamEvent::ControllerEvent(ControllerEvent {
                            id: Uuid::new_v4().to_string(),
                            event_type: ControllerEventType::ActionsQueued,
                            controller_token: Some(token_uuid.to_string()),
                            timestamp_ms: now_ms(),
                            reason: None,
                            controller_account_id: lease
                                .controller_account_id
                                .map(|u| u.to_string()),
                            issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                        }),
                    )
                    .await;
                    return Ok(());
                }

                self.enqueue_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    actions.clone(),
                )
                .await?;

                // Metrics: enqueue count and queue depth gauge
                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                let labels = [label0.as_str(), label1.as_str()];
                metrics::ACTIONS_ENQUEUED
                    .with_label_values(&labels)
                    .inc_by(actions.len() as u64);
                let depth = self
                    .pending_actions_count(
                        &identifiers.private_beach_id.to_string(),
                        &session_uuid.to_string(),
                    )
                    .await
                    .unwrap_or(0);
                metrics::QUEUE_DEPTH
                    .with_label_values(&labels)
                    .set(depth as i64);
                // Pending (unacked) lag gauge
                let pending = self
                    .pending_actions_pending_count(
                        &identifiers.private_beach_id.to_string(),
                        &session_uuid.to_string(),
                    )
                    .await
                    .unwrap_or(0);
                metrics::QUEUE_LAG
                    .with_label_values(&labels)
                    .set(pending as i64);

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "actions_queued",
                    Some(lease.id),
                    lease.controller_account_id,
                    actor_account_id,
                    None,
                )
                .await?;
                tx.commit().await?;

                self.fallback.enqueue_actions(session_id, actions).await;

                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::ActionsQueued,
                        controller_token: Some(token_uuid.to_string()),
                        timestamp_ms: now_ms(),
                        reason: None,
                        controller_account_id: lease.controller_account_id.map(|u| u.to_string()),
                        issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                    }),
                )
                .await;

                Ok(())
            }
        }
    }

    pub async fn poll_actions(&self, session_id: &str) -> Result<Vec<ActionCommand>, StateError> {
        match &self.backend {
            Backend::Memory => self.poll_actions_memory(session_id).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let actions = self
                    .drain_actions_redis(
                        &identifiers.private_beach_id.to_string(),
                        &session_uuid.to_string(),
                    )
                    .await?;
                if actions.is_empty() {
                    return Ok(actions);
                }

                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                let labels = [label0.as_str(), label1.as_str()];
                metrics::ACTIONS_DELIVERED
                    .with_label_values(&labels)
                    .inc_by(actions.len() as u64);

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                if let Some(lease) = self
                    .fetch_active_lease_optional(&mut tx, identifiers.session_id)
                    .await?
                {
                    let token_str = lease.id.to_string();
                    self.insert_controller_event(
                        &mut tx,
                        identifiers.session_id,
                        "actions_acked",
                        Some(lease.id),
                        lease.controller_account_id,
                        lease.controller_account_id,
                        None,
                    )
                    .await?;
                    // Publish while we have lease in scope
                    self.publish(
                        session_id,
                        StreamEvent::ControllerEvent(ControllerEvent {
                            id: Uuid::new_v4().to_string(),
                            event_type: ControllerEventType::ActionsAcked,
                            controller_token: Some(token_str),
                            timestamp_ms: now_ms(),
                            reason: None,
                            controller_account_id: lease
                                .controller_account_id
                                .map(|u| u.to_string()),
                            issued_by_account_id: lease
                                .controller_account_id
                                .map(|u| u.to_string()),
                        }),
                    )
                    .await;
                }
                tx.commit().await?;
                self.fallback.clear_pending_actions(session_id).await;
                Ok(actions)
            }
        }
    }

    pub async fn ack_actions(
        &self,
        session_id: &str,
        acks: Vec<ActionAck>,
        _actor_account_id: Option<Uuid>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                self.fallback.remove_actions(session_id, &acks).await;
                Ok(())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                self.ack_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    &acks,
                )
                .await?;
                // Metrics: record latencies for successful acks
                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                for ack in &acks {
                    if matches!(ack.status, AckStatus::Ok) {
                        if let Some(ms) = ack.latency_ms {
                            metrics::ACTION_LATENCY_MS
                                .with_label_values(&[label0.as_str(), label1.as_str()])
                                .observe(ms as f64);
                        }
                    }
                }
                // Update pending gauge after acks
                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                let pending = self
                    .pending_actions_pending_count(&label0, &label1)
                    .await
                    .unwrap_or(0);
                metrics::QUEUE_LAG
                    .with_label_values(&[label0.as_str(), label1.as_str()])
                    .set(pending as i64);
                self.fallback.remove_actions(session_id, &acks).await;
                Ok(())
            }
        }
    }

    pub async fn record_health(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => self.record_health_memory(session_id, heartbeat).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                self.store_health_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    &heartbeat,
                )
                .await?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO session_runtime (session_id, last_health, last_health_at)
                    VALUES ($1, $2, NOW())
                    ON CONFLICT (session_id)
                    DO UPDATE SET last_health = EXCLUDED.last_health, last_health_at = NOW()
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(Json(serde_json::to_value(&heartbeat)?))
                .execute(tx.as_mut())
                .await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "health_reported",
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
                tx.commit().await?;

                self.fallback.store_health(session_id, heartbeat).await;

                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                metrics::HEALTH_REPORTS
                    .with_label_values(&[label0.as_str(), label1.as_str()])
                    .inc();
                Ok(())
            }
        }
    }

    pub async fn record_state(&self, session_id: &str, diff: StateDiff) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => self.record_state_memory(session_id, diff).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                info!(
                    session_id = %session_id,
                    private_beach_id = %identifiers.private_beach_id,
                    sequence = diff.sequence,
                    "recording state diff"
                );
                self.store_state_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    &diff,
                )
                .await?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO session_runtime (session_id, last_state, last_state_at)
                    VALUES ($1, $2, NOW())
                    ON CONFLICT (session_id)
                    DO UPDATE SET last_state = EXCLUDED.last_state, last_state_at = NOW()
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(Json(serde_json::to_value(&diff)?))
                .execute(tx.as_mut())
                .await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "state_updated",
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
                tx.commit().await?;

                let diff_clone = diff.clone();
                self.fallback
                    .store_state(session_id, diff_clone.clone())
                    .await;
                self.publish(session_id, StreamEvent::State(diff_clone))
                    .await;

                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid.to_string();
                metrics::STATE_REPORTS
                    .with_label_values(&[label0.as_str(), label1.as_str()])
                    .inc();
                Ok(())
            }
        }
    }

    pub async fn viewer_passcode(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<Option<String>, StateError> {
        match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                let record = sessions
                    .get(session_id)
                    .ok_or(StateError::SessionNotFound)?;
                if record.private_beach_id != private_beach_id {
                    return Err(StateError::PrivateBeachNotFound);
                }
                Ok(record.viewer_passcode.clone())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let beach_uuid = parse_uuid(private_beach_id, "private_beach_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                if identifiers.private_beach_id != beach_uuid {
                    return Err(StateError::PrivateBeachNotFound);
                }

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &beach_uuid).await?;
                let passcode = sqlx::query_scalar::<_, Option<String>>(
                    r#"
                    SELECT viewer_passcode
                    FROM session_runtime
                    WHERE session_id = $1
                    "#,
                )
                .bind(identifiers.session_id)
                .fetch_optional(tx.as_mut())
                .await?
                .flatten();
                tx.commit().await?;
                Ok(passcode)
            }
        }
    }

    pub async fn spawn_viewer_worker(&self, session_id: &str) -> Result<(), StateError> {
        let (private_beach_id, passcode) = {
            let sessions = self.fallback.sessions.read().await;
            let record = sessions
                .get(session_id)
                .ok_or(StateError::SessionNotFound)?;
            let passcode = match &record.viewer_passcode {
                Some(code) => code.clone(),
                None => {
                    debug!(
                        target = "private_beach",
                        session_id = %session_id,
                        "viewer passcode not set; skipping viewer worker"
                    );
                    return Ok(());
                }
            };
            (record.private_beach_id.clone(), passcode)
        };

        let base_url = self.road_base_url.clone();
        let mut workers = self.viewer_workers.write().await;
        if let Some(existing) = workers.remove(session_id) {
            existing.handle.abort();
        }
        let state_clone = self.clone();
        let session_id_owned = session_id.to_string();
        let handle = tokio::spawn(async move {
            run_viewer_worker(
                state_clone,
                session_id_owned,
                private_beach_id,
                passcode,
                base_url,
            )
            .await;
        });
        workers.insert(session_id.to_string(), ViewerWorker { handle });
        Ok(())
    }

    pub async fn emergency_stop(
        &self,
        session_id: &str,
        actor_account_id: Option<Uuid>,
        reason: Option<String>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                // Clear pending and release controller
                self.fallback.clear_pending_actions(session_id).await;
                self.fallback.clear_controller(session_id).await;
                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::LeaseReleased,
                        controller_token: None,
                        timestamp_ms: now_ms(),
                        reason,
                        controller_account_id: None,
                        issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                    }),
                )
                .await;
                Ok(())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                // Clear Redis queues
                self.clear_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                )
                .await?;

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                // Revoke lease
                sqlx::query(
                    r#"
                    UPDATE controller_lease
                    SET revoked_at = NOW(), expires_at = NOW()
                    WHERE session_id = $1
                    "#,
                )
                .bind(identifiers.session_id)
                .execute(tx.as_mut())
                .await?;

                // Audit event
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_released",
                    None,
                    None,
                    actor_account_id,
                    reason.clone(),
                )
                .await?;

                tx.commit().await?;

                self.fallback.clear_pending_actions(session_id).await;
                self.fallback.clear_controller(session_id).await;

                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::LeaseReleased,
                        controller_token: None,
                        timestamp_ms: now_ms(),
                        reason,
                        controller_account_id: None,
                        issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                    }),
                )
                .await;
                Ok(())
            }
        }
    }
    pub async fn controller_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<ControllerEvent>, StateError> {
        match &self.backend {
            Backend::Memory => self.controller_events_memory(session_id).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                let rows: Vec<ControllerEventRow> = sqlx::query_as(
                    r#"
                    SELECT id, event_type, controller_token, reason, occurred_at, controller_account_id, issued_by_account_id
                    FROM controller_event
                    WHERE session_id = $1
                    ORDER BY occurred_at DESC
                    LIMIT 200
                    "#,
                )
                .bind(identifiers.session_id)
                .fetch_all(tx.as_mut())
                .await?;

                let events = rows
                    .into_iter()
                    .map(|row| ControllerEvent {
                        id: row.id.to_string(),
                        event_type: controller_event_from_str(&row.event_type),
                        controller_token: row.controller_token.map(|uuid| uuid.to_string()),
                        timestamp_ms: row.occurred_at.timestamp_millis(),
                        reason: row.reason,
                        controller_account_id: row.controller_account_id.map(|u| u.to_string()),
                        issued_by_account_id: row.issued_by_account_id.map(|u| u.to_string()),
                    })
                    .collect();
                tx.commit().await?;
                Ok(events)
            }
        }
    }

    pub async fn controller_events_filtered(
        &self,
        session_id: &str,
        event_type: Option<String>,
        since_ms: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ControllerEvent>, StateError> {
        match &self.backend {
            Backend::Memory => self.controller_events(session_id).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let mut sql = String::from(
                    "SELECT id, event_type, controller_token, reason, occurred_at, controller_account_id, issued_by_account_id FROM controller_event WHERE session_id = $1",
                );
                let mut idx = 2;
                if event_type.is_some() {
                    sql.push_str(" AND event_type = $2::controller_event_type");
                    idx += 1;
                }
                if since_ms.is_some() {
                    let pos = if event_type.is_some() { 3 } else { 2 };
                    sql.push_str(&format!(
                        " AND occurred_at >= to_timestamp(${} / 1000.0)",
                        pos
                    ));
                }
                sql.push_str(" ORDER BY occurred_at DESC LIMIT $X");
                // replace $X with next index
                let lim_idx = if event_type.is_some() && since_ms.is_some() {
                    4
                } else if event_type.is_some() || since_ms.is_some() {
                    3
                } else {
                    2
                };
                let sql = sql.replace("$X", &format!("${}", lim_idx));

                let mut query =
                    sqlx::query_as::<_, ControllerEventRow>(&sql).bind(identifiers.session_id);
                if let Some(et) = event_type {
                    query = query.bind(et);
                }
                if let Some(since) = since_ms {
                    query = query.bind(since);
                }
                query = query.bind(limit as i64);
                let rows = query.fetch_all(pool).await?;
                let events = rows
                    .into_iter()
                    .map(|row| ControllerEvent {
                        id: row.id.to_string(),
                        event_type: controller_event_from_str(&row.event_type),
                        controller_token: row.controller_token.map(|uuid| uuid.to_string()),
                        timestamp_ms: row.occurred_at.timestamp_millis(),
                        reason: row.reason,
                        controller_account_id: row.controller_account_id.map(|u| u.to_string()),
                        issued_by_account_id: row.issued_by_account_id.map(|u| u.to_string()),
                    })
                    .collect();
                Ok(events)
            }
        }
    }

    pub async fn onboard_agent(
        &self,
        session_id: &str,
        template_id: &str,
        scoped_roles: Vec<String>,
        options: HashMap<String, serde_json::Value>,
    ) -> Result<AgentOnboardResponse, StateError> {
        match &self.backend {
            Backend::Memory => {
                self.onboard_agent_memory(session_id, template_id, scoped_roles, options)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                let prompt_pack = serde_json::json!({
                    "template_id": template_id,
                    "session_id": session_uuid,
                    "private_beach_id": identifiers.private_beach_id,
                    "instructions": "You are the designated automation manager for this Private Beach. Follow lease rules and only execute authorized actions.",
                    "scoped_roles": scoped_roles,
                    "options": options,
                });
                let response = AgentOnboardResponse {
                    agent_token: Uuid::new_v4().to_string(),
                    prompt_pack,
                    mcp_bridges: vec![
                        McpBridge {
                            id: "beach_state".into(),
                            name: "Beach State".into(),
                            description: "Read the latest terminal/GUI state for any session"
                                .into(),
                            endpoint: Some("private_beach.subscribe_state".into()),
                        },
                        McpBridge {
                            id: "beach_action".into(),
                            name: "Beach Action".into(),
                            description:
                                "Send actions (keystrokes, pointer events) to sessions you control"
                                    .into(),
                            endpoint: Some("private_beach.queue_action".into()),
                        },
                    ],
                };
                Ok(response)
            }
        }
    }
}

impl InnerState {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    async fn ensure_session(
        &self,
        req: &RegisterSessionRequest,
        harness_id: &str,
        controller_token: Option<String>,
    ) {
        let mut sessions = self.sessions.write().await;
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(&req.session_id, &req.private_beach_id, &req.harness_type)
        });
        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();
        entry.harness_id = harness_id.to_string();
        entry.controller_token = controller_token;
        entry.viewer_passcode = req.viewer_passcode.clone();
    }

    async fn acknowledge_controller(&self, session_id: &str, token: String, ttl: u64) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.controller_token = Some(token);
            record.lease_ttl_ms = ttl;
        }
    }

    async fn clear_controller(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.controller_token = None;
        }
    }

    async fn enqueue_actions(&self, session_id: &str, actions: Vec<ActionCommand>) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            for action in actions {
                record.pending_actions.push_back(action);
            }
        }
    }

    async fn remove_actions(&self, session_id: &str, acks: &[ActionAck]) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            let ack_ids: HashSet<String> = acks
                .iter()
                .filter(|ack| matches!(ack.status, AckStatus::Ok))
                .map(|ack| ack.id.clone())
                .collect();
            if ack_ids.is_empty() {
                return;
            }
            record
                .pending_actions
                .retain(|action| !ack_ids.contains(&action.id));
        }
    }

    async fn clear_pending_actions(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.pending_actions.clear();
        }
    }

    async fn store_health(&self, session_id: &str, heartbeat: HealthHeartbeat) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.last_health = Some(heartbeat);
        }
    }

    async fn store_state(&self, session_id: &str, diff: StateDiff) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.last_state = Some(diff);
        }
    }
}

impl SessionRecord {
    fn new(session_id: &str, private_beach_id: &str, harness_type: &HarnessType) -> Self {
        let harness_id = Uuid::new_v4().to_string();
        let transport_hints = default_transport_hints(session_id);
        Self {
            session_id: session_id.to_string(),
            private_beach_id: private_beach_id.to_string(),
            harness_type: harness_type.clone(),
            capabilities: Vec::new(),
            location_hint: None,
            metadata: None,
            version: "unknown".into(),
            harness_id,
            controller_token: None,
            viewer_passcode: None,
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            transport_hints,
            state_cache_url: None,
            pending_actions: VecDeque::new(),
            controller_events: Vec::new(),
            last_health: None,
            last_state: None,
        }
    }

    fn append_event(&mut self, event_type: ControllerEventType, reason: Option<String>) {
        self.controller_events.push(ControllerEvent {
            id: Uuid::new_v4().to_string(),
            event_type,
            controller_token: self.controller_token.clone(),
            timestamp_ms: now_ms(),
            reason,
            controller_account_id: None,
            issued_by_account_id: None,
        });
    }
}

impl SessionSummary {
    fn from_record(record: &SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            private_beach_id: record.private_beach_id.clone(),
            harness_type: record.harness_type.clone(),
            capabilities: record.capabilities.clone(),
            location_hint: record.location_hint.clone(),
            metadata: record.metadata.clone(),
            version: record.version.clone(),
            harness_id: record.harness_id.clone(),
            controller_token: record.controller_token.clone(),
            controller_expires_at_ms: record
                .controller_token
                .as_ref()
                .map(|_| now_ms() + record.lease_ttl_ms as i64),
            pending_actions: record.pending_actions.len(),
            pending_unacked: record.pending_actions.len(),
            last_health: record.last_health.clone(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_uuid(value: &str, label: &str) -> Result<Uuid, StateError> {
    Uuid::parse_str(value).map_err(|_| StateError::InvalidIdentifier(format!("{label}={value}")))
}

#[derive(sqlx::Type, Debug, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "harness_type", rename_all = "snake_case")]
enum HarnessTypeDb {
    TerminalShim,
    CabanaAdapter,
    RemoteWidget,
    ServiceProxy,
    Custom,
}

impl From<HarnessTypeDb> for HarnessType {
    fn from(db: HarnessTypeDb) -> Self {
        match db {
            HarnessTypeDb::TerminalShim => HarnessType::TerminalShim,
            HarnessTypeDb::CabanaAdapter => HarnessType::CabanaAdapter,
            HarnessTypeDb::RemoteWidget => HarnessType::RemoteWidget,
            HarnessTypeDb::ServiceProxy => HarnessType::ServiceProxy,
            HarnessTypeDb::Custom => HarnessType::Custom,
        }
    }
}

impl From<HarnessType> for HarnessTypeDb {
    fn from(ht: HarnessType) -> Self {
        match ht {
            HarnessType::TerminalShim => HarnessTypeDb::TerminalShim,
            HarnessType::CabanaAdapter => HarnessTypeDb::CabanaAdapter,
            HarnessType::RemoteWidget => HarnessTypeDb::RemoteWidget,
            HarnessType::ServiceProxy => HarnessTypeDb::ServiceProxy,
            HarnessType::Custom => HarnessTypeDb::Custom,
        }
    }
}

fn harness_to_session_kind(value: &HarnessType) -> &'static str {
    match value {
        HarnessType::TerminalShim => "terminal",
        HarnessType::CabanaAdapter => "cabana_gui",
        HarnessType::RemoteWidget => "widget",
        HarnessType::ServiceProxy => "service_daemon",
        HarnessType::Custom => "widget",
    }
}

fn default_transport_hints(session_id: &str) -> HashMap<String, serde_json::Value> {
    let mut hints = HashMap::new();
    hints.insert(
        "actions_poll".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/actions/poll") }),
    );
    hints.insert(
        "actions_ack".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/actions/ack") }),
    );
    hints.insert(
        "state_post".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/state") }),
    );
    hints.insert(
        "health_post".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/health") }),
    );
    // Fast-path (WebRTC) placeholder: surfaced to harness/clients so they can
    // discover the experimental lane. Negotiation details documented in
    // docs/private-beach/beach-manager.md and secure-webrtc plans.
    hints.insert(
        "fast_path_webrtc".into(),
        serde_json::json!({ "status": "planned" }),
    );
    hints
}

fn controller_event_from_str(value: &str) -> ControllerEventType {
    match value {
        "lease_acquired" => ControllerEventType::LeaseAcquired,
        "lease_released" => ControllerEventType::LeaseReleased,
        "actions_queued" => ControllerEventType::ActionsQueued,
        "actions_acked" => ControllerEventType::ActionsAcked,
        "health_reported" => ControllerEventType::HealthReported,
        "state_updated" => ControllerEventType::StateUpdated,
        _ => ControllerEventType::Registered,
    }
}

fn json_array_to_strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn is_active_lease(expires_at: Option<DateTime<Utc>>, revoked_at: Option<DateTime<Utc>>) -> bool {
    let now = Utc::now();
    revoked_at.is_none() && expires_at.map(|exp| exp > now).unwrap_or(false)
}

fn redis_actions_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:actions")
}

fn redis_health_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:health")
}

fn redis_state_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:state")
}

fn redis_action_index_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:actions:index")
}

impl AppState {
    async fn register_session_memory(
        &self,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(&req.session_id, &req.private_beach_id, &req.harness_type)
        });

        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();
        entry.viewer_passcode = req.viewer_passcode.clone();

        if entry.controller_token.is_none() {
            entry.controller_token = Some(Uuid::new_v4().to_string());
        }

        entry.append_event(ControllerEventType::Registered, None);

        let response = RegisterSessionResponse {
            harness_id: entry.harness_id.clone(),
            controller_token: entry.controller_token.clone(),
            lease_ttl_ms: entry.lease_ttl_ms,
            state_cache_url: entry.state_cache_url.clone(),
            transport_hints: entry.transport_hints.clone(),
        };

        drop(sessions);

        if let Err(err) = self.spawn_viewer_worker(&req.session_id).await {
            warn!(
                target = "private_beach",
                session_id = %req.session_id,
                error = %err,
                "failed to start manager viewer worker"
            );
        }

        Ok(response)
    }

    async fn register_session_postgres(
        &self,
        pool: &PgPool,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        let session_uuid = parse_uuid(&req.session_id, "session_id")?;
        let private_beach_uuid = parse_uuid(&req.private_beach_id, "private_beach_id")?;
        let harness_id = Uuid::new_v4();
        let controller_token = Uuid::new_v4();
        let kind = harness_to_session_kind(&req.harness_type);
        let transport_hints = default_transport_hints(&req.session_id);
        let transport_json = serde_json::to_value(&transport_hints)?;
        let capabilities = serde_json::Value::Array(
            req.capabilities
                .iter()
                .map(|cap| serde_json::Value::String(cap.clone()))
                .collect(),
        );
        let metadata = req
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let expires_at = Utc::now() + Duration::milliseconds(DEFAULT_LEASE_TTL_MS as i64);

        let mut tx = pool.begin().await?;
        // Set RLS context to the target private beach for the duration of this registration.
        self.set_rls_context_tx(&mut tx, &private_beach_uuid)
            .await?;

        let session_row = sqlx::query(
            r#"
            INSERT INTO session (
                id, private_beach_id, origin_session_id, harness_id, kind,
                location_hint, capabilities, metadata, harness_type, last_seen_at
            )
            VALUES ($1, $2, $3, $4, $5::session_kind, $6, $7, $8, $9::harness_type, NOW())
            ON CONFLICT (private_beach_id, origin_session_id)
            DO UPDATE SET
                harness_id = EXCLUDED.harness_id,
                kind = EXCLUDED.kind,
                location_hint = EXCLUDED.location_hint,
                capabilities = EXCLUDED.capabilities,
                metadata = EXCLUDED.metadata,
                harness_type = EXCLUDED.harness_type,
                last_seen_at = NOW()
            RETURNING id
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(private_beach_uuid)
        .bind(session_uuid)
        .bind(harness_id)
        .bind(kind)
        .bind(req.location_hint.clone())
        .bind(Json(capabilities))
        .bind(Json(metadata))
        .bind(HarnessTypeDb::from(req.harness_type.clone()))
        .fetch_one(tx.as_mut())
        .await?;
        let db_session_id: Uuid = session_row.try_get("id")?;

        sqlx::query(
            r#"
            INSERT INTO session_runtime (session_id, transport_hints, viewer_passcode)
            VALUES ($1, $2, $3)
            ON CONFLICT (session_id)
            DO UPDATE SET
                transport_hints = EXCLUDED.transport_hints,
                viewer_passcode = EXCLUDED.viewer_passcode
            "#,
        )
        .bind(db_session_id)
        .bind(Json(transport_json))
        .bind(req.viewer_passcode.clone())
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO controller_lease (
                id, session_id, controller_account_id, issued_by_account_id,
                reason, issued_at, expires_at, revoked_at
            )
            VALUES ($1, $2, NULL, NULL, NULL, NOW(), $3, NULL)
            ON CONFLICT (session_id)
            DO UPDATE SET
                id = EXCLUDED.id,
                controller_account_id = NULL,
                issued_by_account_id = NULL,
                reason = NULL,
                issued_at = NOW(),
                expires_at = EXCLUDED.expires_at,
                revoked_at = NULL
            "#,
        )
        .bind(controller_token)
        .bind(db_session_id)
        .bind(expires_at)
        .execute(tx.as_mut())
        .await?;

        self.insert_controller_event(
            &mut tx,
            db_session_id,
            "registered",
            Some(controller_token),
            None,
            None,
            None,
        )
        .await?;

        self.insert_controller_event(
            &mut tx,
            db_session_id,
            "lease_acquired",
            Some(controller_token),
            None,
            None,
            None,
        )
        .await?;

        tx.commit().await?;

        self.fallback
            .ensure_session(
                &req,
                &harness_id.to_string(),
                Some(controller_token.to_string()),
            )
            .await;

        if let Err(err) = self.spawn_viewer_worker(&req.session_id).await {
            warn!(
                target = "private_beach",
                session_id = %req.session_id,
                error = %err,
                "failed to start manager viewer worker"
            );
        }

        info!(
            session_id = %req.session_id,
            private_beach_id = %req.private_beach_id,
            harness_id = %harness_id,
            "session registered with manager"
        );

        Ok(RegisterSessionResponse {
            harness_id: harness_id.to_string(),
            controller_token: Some(controller_token.to_string()),
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            state_cache_url: None,
            transport_hints,
        })
    }

    async fn acquire_controller_memory(
        &self,
        session_id: &str,
        ttl_override: Option<u64>,
        reason: Option<String>,
    ) -> Result<ControllerLeaseResponse, StateError> {
        let ttl = ttl_override.unwrap_or(DEFAULT_LEASE_TTL_MS);
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;

        record.lease_ttl_ms = ttl;
        record.controller_token = Some(Uuid::new_v4().to_string());
        record.append_event(ControllerEventType::LeaseAcquired, reason);

        Ok(ControllerLeaseResponse {
            controller_token: record.controller_token.clone().unwrap(),
            expires_at_ms: now_ms() + ttl as i64,
        })
    }

    async fn release_controller_memory(
        &self,
        session_id: &str,
        controller_token: &str,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        if record.controller_token.as_deref() != Some(controller_token) {
            return Err(StateError::ControllerMismatch);
        }
        record.controller_token = None;
        record.append_event(ControllerEventType::LeaseReleased, None);
        self.publish(
            session_id,
            StreamEvent::ControllerEvent(ControllerEvent {
                id: Uuid::new_v4().to_string(),
                event_type: ControllerEventType::LeaseReleased,
                controller_token: None,
                timestamp_ms: now_ms(),
                reason: None,
                controller_account_id: None,
                issued_by_account_id: None,
            }),
        )
        .await;
        Ok(())
    }

    async fn queue_actions_memory(
        &self,
        session_id: &str,
        controller_token: &str,
        actions: Vec<ActionCommand>,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        if record.controller_token.as_deref() != Some(controller_token) {
            return Err(StateError::ControllerMismatch);
        }
        for action in actions {
            record.pending_actions.push_back(action);
        }
        record.append_event(ControllerEventType::ActionsQueued, None);
        self.publish(
            session_id,
            StreamEvent::ControllerEvent(ControllerEvent {
                id: Uuid::new_v4().to_string(),
                event_type: ControllerEventType::ActionsQueued,
                controller_token: record.controller_token.clone(),
                timestamp_ms: now_ms(),
                reason: None,
                controller_account_id: None,
                issued_by_account_id: None,
            }),
        )
        .await;
        Ok(())
    }

    async fn poll_actions_memory(
        &self,
        session_id: &str,
    ) -> Result<Vec<ActionCommand>, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let actions: Vec<ActionCommand> = record.pending_actions.drain(..).collect();
        if !actions.is_empty() {
            self.publish(
                session_id,
                StreamEvent::ControllerEvent(ControllerEvent {
                    id: Uuid::new_v4().to_string(),
                    event_type: ControllerEventType::ActionsAcked,
                    controller_token: record.controller_token.clone(),
                    timestamp_ms: now_ms(),
                    reason: None,
                    controller_account_id: None,
                    issued_by_account_id: None,
                }),
            )
            .await;
        }
        Ok(actions)
    }

    async fn record_health_memory(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        record.last_health = Some(heartbeat.clone());
        record.append_event(ControllerEventType::HealthReported, None);
        self.publish(session_id, StreamEvent::Health(heartbeat))
            .await;
        Ok(())
    }

    async fn record_state_memory(
        &self,
        session_id: &str,
        diff: StateDiff,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let diff_clone = diff.clone();
        self.store_state_redis(&record.private_beach_id, session_id, &diff_clone)
            .await?;
        record.last_state = Some(diff);
        record.append_event(ControllerEventType::StateUpdated, None);
        self.publish(session_id, StreamEvent::State(diff_clone))
            .await;
        Ok(())
    }

    async fn controller_events_memory(
        &self,
        session_id: &str,
    ) -> Result<Vec<ControllerEvent>, StateError> {
        let sessions = self.fallback.sessions.read().await;
        let record = sessions
            .get(session_id)
            .ok_or(StateError::SessionNotFound)?;
        Ok(record.controller_events.clone())
    }

    async fn onboard_agent_memory(
        &self,
        session_id: &str,
        template_id: &str,
        scoped_roles: Vec<String>,
        options: HashMap<String, serde_json::Value>,
    ) -> Result<AgentOnboardResponse, StateError> {
        let sessions = self.fallback.sessions.read().await;
        let record = sessions
            .get(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let prompt_pack = serde_json::json!({
            "template_id": template_id,
            "session_id": record.session_id,
            "private_beach_id": record.private_beach_id,
            "instructions": "You are the designated automation manager for this Private Beach. Follow lease rules and only execute authorized actions.",
            "scoped_roles": scoped_roles,
            "options": options,
        });
        Ok(AgentOnboardResponse {
            agent_token: Uuid::new_v4().to_string(),
            prompt_pack,
            mcp_bridges: vec![
                McpBridge {
                    id: "beach_state".into(),
                    name: "Beach State".into(),
                    description: "Read the latest terminal/GUI state for any session".into(),
                    endpoint: Some("private_beach.subscribe_state".into()),
                },
                McpBridge {
                    id: "beach_action".into(),
                    name: "Beach Action".into(),
                    description:
                        "Send actions (keystrokes, pointer events) to sessions you control".into(),
                    endpoint: Some("private_beach.queue_action".into()),
                },
            ],
        })
    }

    async fn fetch_session_identifiers(
        &self,
        pool: &PgPool,
        session_uuid: &Uuid,
    ) -> Result<DbSessionIdentifiers, StateError> {
        let row = sqlx::query_as::<_, DbSessionIdentifiers>(
            r#"
            SELECT id AS session_id, private_beach_id
            FROM session
            WHERE origin_session_id = $1
            "#,
        )
        .bind(session_uuid)
        .fetch_optional(pool)
        .await?;

        row.ok_or(StateError::SessionNotFound)
    }

    async fn fetch_active_lease(
        &self,
        pool: &PgPool,
        session_id: Uuid,
    ) -> Result<LeaseRow, StateError> {
        let row: Option<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

        row.ok_or(StateError::ControllerMismatch)
    }

    async fn fetch_active_lease_optional(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
    ) -> Result<Option<LeaseRow>, StateError> {
        let row: Option<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?;

        Ok(row)
    }

    async fn insert_controller_event(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
        event_type: &str,
        controller_token: Option<Uuid>,
        controller_account_id: Option<Uuid>,
        issued_by_account_id: Option<Uuid>,
        reason: Option<String>,
    ) -> Result<(), StateError> {
        sqlx::query(
            r#"
            INSERT INTO controller_event (
                id, session_id, event_type, controller_token, controller_account_id,
                issued_by_account_id, reason, occurred_at
            )
            VALUES ($1, $2, $3::controller_event_type, $4, $5, $6, $7, NOW())
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(event_type)
        .bind(controller_token)
        .bind(controller_account_id)
        .bind(issued_by_account_id)
        .bind(reason)
        .execute(tx.as_mut())
        .await?;
        Ok(())
    }

    async fn set_rls_context_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        private_beach_id: &Uuid,
    ) -> Result<(), StateError> {
        sqlx::query("SELECT set_config('beach.private_beach_id', $1, true)")
            .bind(private_beach_id.to_string())
            .execute(tx.as_mut())
            .await?;
        Ok(())
    }

    async fn set_account_context_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        account_id: Option<&Uuid>,
    ) -> Result<(), StateError> {
        let value = account_id.map(|u| u.to_string()).unwrap_or_default();
        sqlx::query("SELECT set_config('beach.account_id', $1, true)")
            .bind(value)
            .execute(tx.as_mut())
            .await?;
        Ok(())
    }

    async fn enqueue_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        actions: Vec<ActionCommand>,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let index_key = redis_action_index_key(private_beach_id, session_id);

            if let Err(err) = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(&key)
                .arg(REDIS_ACTION_GROUP)
                .arg("0")
                .arg("MKSTREAM")
                .query_async::<_, ()>(&mut conn)
                .await
            {
                if err.code() != Some("BUSYGROUP") {
                    return Err(StateError::Redis(err));
                }
            }

            for action in &actions {
                let payload = serde_json::to_string(action)?;
                let entry_id: String = redis::cmd("XADD")
                    .arg(&key)
                    .arg("MAXLEN")
                    .arg("~")
                    .arg(REDIS_ACTION_STREAM_MAXLEN)
                    .arg("*")
                    .arg("action_id")
                    .arg(&action.id)
                    .arg("payload")
                    .arg(payload)
                    .query_async(&mut conn)
                    .await?;

                redis::cmd("HSET")
                    .arg(&index_key)
                    .arg(&action.id)
                    .arg(&entry_id)
                    .query_async::<_, ()>(&mut conn)
                    .await?;
            }
            redis::cmd("EXPIRE")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .query_async::<_, ()>(&mut conn)
                .await?;
            redis::cmd("EXPIRE")
                .arg(&index_key)
                .arg(REDIS_TTL_SECONDS)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn drain_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<Vec<ActionCommand>, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let consumer = format!("{REDIS_ACTION_CONSUMER_PREFIX}:{session_id}");

            let value: redis::Value = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(REDIS_ACTION_GROUP)
                .arg(&consumer)
                .arg("COUNT")
                .arg(64)
                .arg("STREAMS")
                .arg(&key)
                .arg(">")
                .query_async(&mut conn)
                .await?;

            let mut actions = Vec::new();

            if !matches!(value, redis::Value::Nil) {
                let streams: Vec<(String, Vec<(String, Vec<(String, String)>)>)> =
                    redis::from_redis_value(&value)?;
                for (_stream, entries) in streams {
                    for (_entry_id, fields) in entries {
                        for (field, value) in fields {
                            if field == "payload" {
                                let action: ActionCommand = serde_json::from_str(&value)?;
                                actions.push(action);
                            }
                        }
                    }
                }
            }

            return Ok(actions);
        }
        let sessions = self.fallback.sessions.read().await;
        Ok(sessions
            .get(session_id)
            .map(|record| record.pending_actions.iter().cloned().collect())
            .unwrap_or_default())
    }

    async fn pending_actions_count(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<usize, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let len: usize = redis::cmd("XLEN")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(0);
            return Ok(len);
        }
        let sessions = self.fallback.sessions.read().await;
        Ok(sessions
            .get(session_id)
            .map(|record| record.pending_actions.len())
            .unwrap_or(0))
    }

    async fn pending_actions_pending_count(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<usize, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            // XPENDING <key> <group>
            let value: redis::Value = redis::cmd("XPENDING")
                .arg(&key)
                .arg(REDIS_ACTION_GROUP)
                .query_async(&mut conn)
                .await
                .unwrap_or(redis::Value::Nil);
            if let redis::Value::Bulk(items) = value {
                if let Some(redis::Value::Int(count)) = items.get(0) {
                    return Ok((*count).try_into().unwrap_or(0));
                }
            }
            return Ok(0);
        }
        let sessions = self.fallback.sessions.read().await;
        Ok(sessions
            .get(session_id)
            .map(|record| record.pending_actions.len())
            .unwrap_or(0))
    }

    async fn store_health_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        heartbeat: &HealthHeartbeat,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_health_key(private_beach_id, session_id);
            let payload = serde_json::to_string(heartbeat)?;
            redis::cmd("SETEX")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .arg(payload)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn store_state_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        diff: &StateDiff,
    ) -> Result<(), StateError> {
        #[cfg(test)]
        test_support::record_redis_state_write(private_beach_id, session_id, diff);
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_state_key(private_beach_id, session_id);
            let payload = serde_json::to_string(diff)?;
            redis::cmd("SETEX")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .arg(payload)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn ack_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        acks: &[ActionAck],
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let index_key = redis_action_index_key(private_beach_id, session_id);

            for ack in acks {
                if !matches!(ack.status, AckStatus::Ok) {
                    continue;
                }

                let entry_id: Option<String> = redis::cmd("HGET")
                    .arg(&index_key)
                    .arg(&ack.id)
                    .query_async(&mut conn)
                    .await?;

                if let Some(entry_id) = entry_id {
                    redis::cmd("XACK")
                        .arg(&key)
                        .arg(REDIS_ACTION_GROUP)
                        .arg(&entry_id)
                        .query_async::<_, i64>(&mut conn)
                        .await?;
                    redis::cmd("XDEL")
                        .arg(&key)
                        .arg(&entry_id)
                        .query_async::<_, i64>(&mut conn)
                        .await?;
                    redis::cmd("HDEL")
                        .arg(&index_key)
                        .arg(&ack.id)
                        .query_async::<_, i64>(&mut conn)
                        .await?;
                }
            }

            redis::cmd("EXPIRE")
                .arg(&index_key)
                .arg(REDIS_TTL_SECONDS)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn clear_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let index_key = redis_action_index_key(private_beach_id, session_id);
            let _: () = redis::cmd("DEL")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(());
            let _: () = redis::cmd("DEL")
                .arg(&index_key)
                .query_async(&mut conn)
                .await
                .unwrap_or(());
        }
        Ok(())
    }
}

// ---- Private Beaches: CRUD + layout ----
impl AppState {
    pub async fn create_private_beach(
        &self,
        name: &str,
        slug: Option<&str>,
        owner: Option<Uuid>,
    ) -> Result<crate::routes::BeachSummary, StateError> {
        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => {
                return Err(StateError::Database(sqlx::Error::Protocol(
                    "requires postgres backend".into(),
                )));
            }
        };

        let mut tx = pool.begin().await?;
        if let Some(owner_id) = owner {
            self.set_account_context_tx(&mut tx, Some(&owner_id))
                .await?;
        } else {
            self.set_account_context_tx(&mut tx, None).await?;
        }

        let base_slug = slug.map(|s| s.to_string()).unwrap_or_else(|| slugify(name));

        // Try up to 5 variants to avoid slug collisions
        let mut final_row: Option<(Uuid, String, String, chrono::DateTime<Utc>)> = None;
        for i in 0..5 {
            let candidate = if i == 0 {
                base_slug.clone()
            } else {
                format!("{}-{}", base_slug, &Uuid::new_v4().to_string()[..8])
            };
            let res = sqlx::query_as::<_, (Uuid, String, String, chrono::DateTime<Utc>)>(
                r#"
                INSERT INTO private_beach (name, slug, owner_account_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (slug) DO NOTHING
                RETURNING id, name, slug, created_at
                "#,
            )
            .bind(name)
            .bind(&candidate)
            .bind(owner)
            .fetch_optional(tx.as_mut())
            .await?;

            if let Some(row) = res {
                final_row = Some(row);
                break;
            }
        }

        let (id, name, slug, created_at) = final_row.ok_or_else(|| {
            StateError::Database(sqlx::Error::Protocol(
                "could not allocate unique slug".into(),
            ))
        })?;

        // Insert owner membership when available (best-effort; RLS requires account + beach GUCs)
        if let Some(owner_id) = owner {
            self.set_rls_context_tx(&mut tx, &id).await?;
            let _ = sqlx::query(
                r#"
                INSERT INTO private_beach_membership (private_beach_id, account_id, role, status, invited_by_account_id, invited_at, activated_at)
                VALUES ($1, $2, 'owner', 'active', $2, NOW(), NOW())
                ON CONFLICT (private_beach_id, account_id) DO NOTHING
                "#,
            )
            .bind(id)
            .bind(owner_id)
            .execute(tx.as_mut())
            .await?;
        }

        tx.commit().await?;
        Ok(crate::routes::BeachSummary {
            id: id.to_string(),
            name,
            slug,
            created_at: created_at.timestamp_millis(),
        })
    }

    pub async fn list_private_beaches(
        &self,
        account: Option<Uuid>,
    ) -> Result<Vec<crate::routes::BeachSummary>, StateError> {
        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => return Ok(Vec::new()),
        };
        let mut tx = pool.begin().await?;
        if let Some(a) = account {
            self.set_account_context_tx(&mut tx, Some(&a)).await?;
        } else {
            self.set_account_context_tx(&mut tx, None).await?;
        }
        let rows: Vec<(Uuid, String, String, chrono::DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT id, name, slug, created_at
            FROM private_beach
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(tx.as_mut())
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, slug, created_at)| crate::routes::BeachSummary {
                id: id.to_string(),
                name,
                slug,
                created_at: created_at.timestamp_millis(),
            })
            .collect())
    }

    pub async fn get_private_beach(
        &self,
        id_str: &str,
        account: Option<Uuid>,
    ) -> Result<crate::routes::BeachMeta, StateError> {
        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => return Err(StateError::PrivateBeachNotFound),
        };
        let id = parse_uuid(id_str, "id")?;
        let mut tx = pool.begin().await?;
        if let Some(a) = account {
            self.set_account_context_tx(&mut tx, Some(&a)).await?;
        } else {
            self.set_account_context_tx(&mut tx, None).await?;
        }
        // Also set beach GUC to allow bypass mode SELECT for ownerless rows via policy OR clause
        self.set_rls_context_tx(&mut tx, &id).await?;
        let row: Option<(
            Uuid,
            String,
            String,
            serde_json::Value,
            chrono::DateTime<Utc>,
        )> = sqlx::query_as(
            r#"
            SELECT id, name, slug, COALESCE(settings, '{}'::jsonb) AS settings, created_at
            FROM private_beach
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(tx.as_mut())
        .await?;
        tx.commit().await?;
        let (id, name, slug, settings, created_at) = row.ok_or(StateError::PrivateBeachNotFound)?;
        Ok(crate::routes::BeachMeta {
            id: id.to_string(),
            name,
            slug,
            settings,
            created_at: created_at.timestamp_millis(),
        })
    }

    pub async fn update_private_beach(
        &self,
        id_str: &str,
        name: Option<&str>,
        slug: Option<&str>,
        settings: Option<serde_json::Value>,
        account: Option<Uuid>,
    ) -> Result<crate::routes::BeachMeta, StateError> {
        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => return Err(StateError::PrivateBeachNotFound),
        };
        let id = parse_uuid(id_str, "id")?;
        let mut tx = pool.begin().await?;
        if let Some(a) = account {
            self.set_account_context_tx(&mut tx, Some(&a)).await?;
        } else {
            self.set_account_context_tx(&mut tx, None).await?;
        }
        self.set_rls_context_tx(&mut tx, &id).await?;

        // Authorization: if no account, only allow ownerless beaches (dev bypass)
        if account.is_none() {
            let ok: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM private_beach WHERE id = $1 AND owner_account_id IS NULL",
            )
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await?;
            if ok.is_none() {
                return Err(StateError::PrivateBeachNotFound);
            }
        } else {
            // Ensure caller can see the beach (membership/owner) before update
            let exists: Option<(Uuid,)> =
                sqlx::query_as(r#"SELECT id FROM private_beach WHERE id = $1"#)
                    .bind(id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            if exists.is_none() {
                return Err(StateError::PrivateBeachNotFound);
            }
        }

        let new_name = name.map(|s| s.to_string());
        let new_slug = slug.map(|s| s.to_string());
        let mut parts: Vec<&str> = Vec::new();
        if new_name.is_some() {
            parts.push("name = $2");
        }
        if new_slug.is_some() {
            parts.push("slug = $3");
        }
        if settings.is_some() {
            parts.push("settings = $4");
        }
        if parts.is_empty() {
            drop(tx);
            return self.get_private_beach(id_str, account).await;
        }

        let sql = format!(
            "UPDATE private_beach SET {assigns} , updated_at = NOW() WHERE id = $1 RETURNING id, name, slug, COALESCE(settings, '{{}}'::jsonb) AS settings, created_at",
            assigns = parts.join(", ")
        );
        let row: (
            Uuid,
            String,
            String,
            serde_json::Value,
            chrono::DateTime<Utc>,
        ) = sqlx::query_as(&sql)
            .bind(id)
            .bind(new_name)
            .bind(new_slug)
            .bind(settings)
            .fetch_one(tx.as_mut())
            .await?;
        tx.commit().await?;
        Ok(crate::routes::BeachMeta {
            id: row.0.to_string(),
            name: row.1,
            slug: row.2,
            settings: row.3,
            created_at: row.4.timestamp_millis(),
        })
    }

    pub async fn get_private_beach_layout(
        &self,
        id_str: &str,
        account: Option<Uuid>,
    ) -> Result<crate::routes::BeachLayout, StateError> {
        let meta = self.get_private_beach(id_str, account).await?;
        let layout = meta
            .settings
            .get("layout")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "preset": "grid2x2", "tiles": [] }));
        let preset = layout
            .get("preset")
            .and_then(|v| v.as_str())
            .unwrap_or("grid2x2")
            .to_string();
        let tiles = layout
            .get("tiles")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| Vec::new());
        Ok(crate::routes::BeachLayout { preset, tiles })
    }

    pub async fn put_private_beach_layout(
        &self,
        id_str: &str,
        preset: String,
        tiles: Vec<String>,
        account: Option<Uuid>,
    ) -> Result<(), StateError> {
        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => return Ok(()),
        };
        let id = parse_uuid(id_str, "id")?;
        let mut tx = pool.begin().await?;
        if let Some(a) = account {
            self.set_account_context_tx(&mut tx, Some(&a)).await?;
        } else {
            self.set_account_context_tx(&mut tx, None).await?;
        }
        self.set_rls_context_tx(&mut tx, &id).await?;

        // Authorization: same as update
        if account.is_none() {
            let ok: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM private_beach WHERE id = $1 AND owner_account_id IS NULL",
            )
            .bind(id)
            .fetch_optional(tx.as_mut())
            .await?;
            if ok.is_none() {
                return Err(StateError::PrivateBeachNotFound);
            }
        } else {
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM private_beach WHERE id = $1")
                    .bind(id)
                    .fetch_optional(tx.as_mut())
                    .await?;
            if exists.is_none() {
                return Err(StateError::PrivateBeachNotFound);
            }
        }

        // Upsert settings->'layout'
        let layout = serde_json::json!({ "preset": preset, "tiles": tiles });
        sqlx::query(
            r#"
            UPDATE private_beach
            SET settings = jsonb_set(COALESCE(settings, '{}'::jsonb), '{layout}', $2::jsonb, true),
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(Json(layout).0)
        .execute(tx.as_mut())
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, FromRow)]
struct LeaseRow {
    id: Uuid,
    controller_account_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

async fn run_viewer_worker(
    state: AppState,
    session_id: String,
    private_beach_id: String,
    passcode: String,
    road_base_url: String,
) {
    #[cfg(test)]
    if let Some(override_fn) = test_support::viewer_worker_override() {
        override_fn(state, session_id, private_beach_id, passcode, road_base_url).await;
        return;
    }
    let _ = &state;
    let label = "beach-manager";
    let label_private = private_beach_id.clone();
    let label_session = session_id.clone();
    metrics::MANAGER_VIEWER_CONNECTED
        .with_label_values(&[label_private.as_str(), label_session.as_str()])
        .set(0);
    let mut attempts: usize = 0;
    loop {
        if attempts > 0 {
            metrics::MANAGER_VIEWER_RECONNECTS
                .with_label_values(&[label_private.as_str(), label_session.as_str()])
                .inc();
        }
        attempts = attempts.saturating_add(1);
        match viewer_connect_once(
            &state,
            &session_id,
            &private_beach_id,
            &passcode,
            &road_base_url,
            label,
        )
        .await
        {
            Ok(()) => {
                info!(
                    target = "private_beach",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    "manager viewer disconnected cleanly"
                );
            }
            Err(err) => {
                warn!(
                    target = "private_beach",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    error = %err,
                    "manager viewer connection failed"
                );
            }
        }
        metrics::MANAGER_VIEWER_CONNECTED
            .with_label_values(&[label_private.as_str(), label_session.as_str()])
            .set(0);
        sleep(StdDuration::from_secs(3)).await;
    }
}

async fn viewer_connect_once(
    state: &AppState,
    session_id: &str,
    private_beach_id: &str,
    passcode: &str,
    road_base_url: &str,
    label: &str,
) -> Result<(), ViewerError> {
    let gauge =
        metrics::MANAGER_VIEWER_CONNECTED.with_label_values(&[private_beach_id, session_id]);
    gauge.set(0);
    let gauge_guard = ViewerGaugeGuard::new(gauge.clone());
    let latency_hist =
        metrics::MANAGER_VIEWER_LATENCY_MS.with_label_values(&[private_beach_id, session_id]);

    let config = SessionConfig::new(road_base_url).map_err(ViewerError::Join)?;
    let manager = SessionManager::new(config).map_err(ViewerError::Join)?;
    let joined = manager
        .join(session_id, passcode, Some(label), false)
        .await
        .map_err(ViewerError::Join)?;
    let negotiated = negotiate_transport(joined.handle(), Some(passcode), Some(label), false)
        .await
        .map_err(ViewerError::Negotiation)?;
    let transport = match negotiated {
        NegotiatedTransport::Single(NegotiatedSingle { transport, .. }) => transport,
        NegotiatedTransport::WebRtcOfferer { .. } => {
            return Err(ViewerError::UnsupportedTransport("offerer"));
        }
    };

    info!(
        target = "private_beach",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        "manager viewer connected via webrtc"
    );
    gauge_guard.mark_connected();

    if let Err(err) = transport.send_text("__ready__") {
        debug!(
            target = "private_beach",
            session_id = %session_id,
            error = %err,
            "manager viewer failed to send ready sentinel"
        );
    }

    let resize_frame = WireClientFrame::Resize {
        cols: 120,
        rows: 40,
    };
    let resize_bytes = encode_client_frame_binary(&resize_frame);
    if let Err(err) = transport.send_bytes(&resize_bytes) {
        debug!(
            target = "private_beach",
            session_id = %session_id,
            error = %err,
            "manager viewer failed to send resize frame"
        );
    }

    let mut viewer_state = ManagerViewerState::new();
    let mut next_keepalive = Instant::now() + VIEWER_KEEPALIVE_INTERVAL;

    loop {
        let now = Instant::now();
        if now >= next_keepalive {
            if let Err(err) = transport.send_text("__ready__") {
                debug!(
                    target = "private_beach",
                    session_id = %session_id,
                    error = %err,
                    "manager viewer keepalive failed"
                );
            }
            next_keepalive = now + VIEWER_KEEPALIVE_INTERVAL;
        }
        match transport.recv(StdDuration::from_millis(500)) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => match decode_host_frame_binary(&bytes) {
                    Ok(frame) => {
                        let frame_type = match &frame {
                            WireHostFrame::Hello { .. } => "hello",
                            WireHostFrame::Grid { .. } => "grid",
                            WireHostFrame::Snapshot { .. } => "snapshot",
                            WireHostFrame::SnapshotComplete { .. } => "snapshot_complete",
                            WireHostFrame::Delta { .. } => "delta",
                            WireHostFrame::HistoryBackfill { .. } => "history_backfill",
                            WireHostFrame::InputAck { .. } => "input_ack",
                            WireHostFrame::Cursor { .. } => "cursor",
                            WireHostFrame::Heartbeat { .. } => "heartbeat",
                            WireHostFrame::Shutdown => "shutdown",
                        };
                        debug!(
                            target = "private_beach",
                            session_id = %session_id,
                            frame = frame_type,
                            "manager viewer received host frame"
                        );
                        if let WireHostFrame::Heartbeat { timestamp_ms, .. } = &frame {
                            let now_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            if now_ms >= *timestamp_ms {
                                let latency_ms = now_ms - *timestamp_ms;
                                latency_hist.observe(latency_ms as f64);
                            }
                        }
                        if let Some(diff) = viewer_state.handle_host_frame(&frame) {
                            let sequence = diff.sequence;
                            if let Err(err) = state.record_state(session_id, diff).await {
                                warn!(
                                    target = "private_beach",
                                    session_id = %session_id,
                                    private_beach_id = %private_beach_id,
                                    error = %err,
                                    sequence,
                                    "manager viewer failed to persist diff"
                                );
                            }
                        }
                        if matches!(frame, WireHostFrame::Shutdown) {
                            info!(
                                target = "private_beach",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                "manager viewer received shutdown frame"
                            );
                            return Ok(());
                        }
                    }
                    Err(err) => {
                        warn!(
                            target = "private_beach",
                            session_id = %session_id,
                            error = %err,
                            "manager viewer failed to decode host frame"
                        );
                    }
                },
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        debug!(
                            target = "private_beach",
                            session_id = %session_id,
                            payload = %trimmed,
                            "manager viewer received text payload"
                        );
                    }
                }
            },
            Err(TransportError::Timeout) => continue,
            Err(TransportError::ChannelClosed) => {
                info!(
                    target = "private_beach",
                    session_id = %session_id,
                    "manager viewer transport closed by peer"
                );
                return Ok(());
            }
            Err(err) => {
                return Err(ViewerError::Transport(err));
            }
        }
    }
}

struct ViewerGaugeGuard {
    gauge: IntGauge,
}

impl ViewerGaugeGuard {
    fn new(gauge: IntGauge) -> Self {
        Self { gauge }
    }

    fn mark_connected(&self) {
        self.gauge.set(1);
    }
}

impl Drop for ViewerGaugeGuard {
    fn drop(&mut self) {
        self.gauge.set(0);
    }
}

#[derive(Debug, thiserror::Error)]
enum ViewerError {
    #[error("session join failed: {0}")]
    Join(SessionError),
    #[error("transport negotiation failed: {0}")]
    Negotiation(CliError),
    #[error("transport error: {0}")]
    Transport(TransportError),
    #[error("unsupported transport: {0}")]
    UnsupportedTransport(&'static str),
}

#[derive(Debug, thiserror::Error)]
pub enum JoinForwardError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvertisedTransportKind {
    WebRtc,
    WebSocket,
    Ipc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvertisedTransport {
    pub kind: AdvertisedTransportKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinSessionResponsePayload {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webrtc_offer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_url: Option<String>,
    #[serde(default)]
    pub transports: Vec<AdvertisedTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_url: Option<String>,
}

fn slugify(name: &str) -> String {
    let mut s = name.to_lowercase();
    s = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    // collapse repeated '-'
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(ch);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use once_cell::sync::Lazy;
    use std::{
        future::Future,
        mem,
        pin::Pin,
        sync::{Arc, RwLock},
    };

    type ViewerOverride = Arc<
        dyn Fn(AppState, String, String, String, String) -> Pin<Box<dyn Future<Output = ()> + Send>>
            + Send
            + Sync,
    >;

    static VIEWER_WORKER_OVERRIDE: Lazy<RwLock<Option<ViewerOverride>>> =
        Lazy::new(|| RwLock::new(None));

    pub fn set_viewer_worker_override<F, Fut>(f: Option<F>)
    where
        F: Fn(AppState, String, String, String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut guard = VIEWER_WORKER_OVERRIDE.write().unwrap();
        *guard = f.map(|func| {
            Arc::new(
                move |state, session_id, private_beach_id, passcode, base_url| {
                    Box::pin(func(
                        state,
                        session_id,
                        private_beach_id,
                        passcode,
                        base_url,
                    )) as Pin<Box<dyn Future<Output = ()> + Send>>
                },
            ) as ViewerOverride
        });
    }

    pub fn viewer_worker_override() -> Option<ViewerOverride> {
        VIEWER_WORKER_OVERRIDE
            .read()
            .unwrap()
            .as_ref()
            .map(Arc::clone)
    }

    pub fn clear_viewer_worker_override() {
        let mut guard = VIEWER_WORKER_OVERRIDE.write().unwrap();
        *guard = None;
    }

    static REDIS_STATE_WRITES: Lazy<RwLock<Vec<(String, String, StateDiff)>>> =
        Lazy::new(|| RwLock::new(Vec::new()));

    pub fn record_redis_state_write(private_beach_id: &str, session_id: &str, diff: &StateDiff) {
        let mut guard = REDIS_STATE_WRITES.write().unwrap();
        guard.push((
            private_beach_id.to_string(),
            session_id.to_string(),
            diff.clone(),
        ));
    }

    pub fn take_redis_state_writes() -> Vec<(String, String, StateDiff)> {
        let mut guard = REDIS_STATE_WRITES.write().unwrap();
        let mut out = Vec::new();
        mem::swap(&mut *guard, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support;
    use beach_buggy::{HarnessType, RegisterSessionRequest};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::SystemTime;
    use tokio::time::{Duration, sleep, timeout};

    #[test_timeout::tokio_timeout_test(10)]
    async fn spawn_viewer_worker_smoke_test_records_state_and_stream() {
        // clear any prior captured redis writes
        let _ = test_support::take_redis_state_writes();
        test_support::clear_viewer_worker_override();
        let _guard = OverrideGuard;

        let invocation_count = Arc::new(AtomicUsize::new(0));
        test_support::set_viewer_worker_override(Some({
            let invocation_count = invocation_count.clone();
            move |state: AppState,
                  session_id: String,
                  private_beach_id: String,
                  passcode: String,
                  _base_url: String| {
                let invocation_count = invocation_count.clone();
                async move {
                    assert_eq!(session_id, "sess-view");
                    assert_eq!(private_beach_id, "pb-view");
                    assert_eq!(passcode, "secret");
                    let diff = beach_buggy::StateDiff {
                        sequence: 1,
                        emitted_at: SystemTime::now(),
                        payload: json!({ "hello": "world" }),
                    };
                    state.record_state(&session_id, diff).await.unwrap();
                    invocation_count.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));

        let state = AppState::new();
        let register = RegisterSessionRequest {
            session_id: "sess-view".into(),
            private_beach_id: "pb-view".into(),
            harness_type: HarnessType::TerminalShim,
            capabilities: vec![],
            location_hint: None,
            metadata: None,
            version: "1.0.0".into(),
            viewer_passcode: Some("secret".into()),
        };
        state.register_session(register).await.unwrap();

        timeout(Duration::from_secs(2), {
            let invocation_count = invocation_count.clone();
            async move {
                while invocation_count.load(Ordering::SeqCst) < 1 {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await
        .expect("initial viewer worker invocation");

        let mut receiver = state.subscribe_session("sess-view").await;

        state.spawn_viewer_worker("sess-view").await.unwrap();

        timeout(Duration::from_secs(2), {
            let invocation_count = invocation_count.clone();
            async move {
                while invocation_count.load(Ordering::SeqCst) < 2 {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await
        .expect("second viewer worker invocation");

        let event = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("viewer worker should emit")
            .expect("stream closed unexpectedly");
        match event {
            StreamEvent::State(diff) => {
                assert_eq!(diff.sequence, 1);
                assert_eq!(diff.payload, json!({ "hello": "world" }));
            }
            other => panic!("expected state event, got {:?}", other),
        }

        let writes = test_support::take_redis_state_writes();
        let matching: Vec<_> = writes
            .into_iter()
            .filter(|(pb, sess, diff)| {
                pb == "pb-view"
                    && sess == "sess-view"
                    && diff.sequence == 1
                    && diff.payload == json!({ "hello": "world" })
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "expected redis capture for viewer diff"
        );
    }

    struct OverrideGuard;

    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            test_support::clear_viewer_worker_override();
        }
    }
}

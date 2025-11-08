//! Control-plane state for the Beach Manager service.
//!
//! The **manager** refers to this Rust control plane (`apps/beach-manager`). A **controller**
//! is any Beach session whose Beach Buggy harness currently holds a controller lease via the
//! manager APIs. Controllers drive other sessions; non-controller harnesses simply stream
//! state into the manager until a lease is granted.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::auth::{AuthConfig, AuthContext};
use crate::fastpath::{send_actions_over_fast_path, FastPathRegistry, FastPathSession};
use crate::metrics;
use beach_buggy::{
    AckStatus, ActionAck, ActionCommand, CellStylePayload, CursorPosition, HarnessType,
    HealthHeartbeat, RegisterSessionRequest, RegisterSessionResponse, StateDiff, StyleDefinition,
    StyledCell, TerminalFrame,
};
use beach_client_core::cache::terminal::packed::unpack_cell;
use beach_client_core::protocol::{ClientFrame, CursorFrame, Update as WireUpdate};
use beach_client_core::{
    decode_host_frame_binary, encode_client_frame_binary, negotiate_transport, CliError,
    HostFrame as WireHostFrame, NegotiatedSingle, NegotiatedTransport, PackedCell, Payload,
    SessionConfig, SessionError, SessionHandle, SessionManager, Style, StyleId, TerminalGrid,
    TransportError, TransportOffer,
};
use chrono::{DateTime, Duration, Utc};
use prometheus::IntGauge;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, FromRow, PgPool, Row};
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, info, trace, warn, Level};
use url::Url;
use uuid::Uuid;

const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
const REDIS_ACTION_STREAM_MAXLEN: usize = 2_048;
const REDIS_TTL_SECONDS: usize = 120;
const REDIS_ACTION_GROUP: &str = "controllers";
const REDIS_ACTION_CONSUMER_PREFIX: &str = "poller";
const VIEWER_KEEPALIVE_INTERVAL: StdDuration = StdDuration::from_secs(20);
const VIEWER_KEEPALIVE_PAYLOAD: &str = "__keepalive__";
const VIEWER_IDLE_LOG_AFTER: StdDuration = StdDuration::from_secs(45);

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
    pairings: RwLock<HashMap<String, Vec<ControllerPairing>>>,
    canvas_layouts: RwLock<HashMap<String, crate::routes::CanvasLayout>>,
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

pub(crate) struct ViewerTokenIssued {
    pub token: String,
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ViewerTokenError {
    #[error("viewer token unauthorized")]
    Unauthorized,
    #[error("viewer token service unavailable")]
    Unavailable,
    #[error("viewer token http error: {0}")]
    Http(reqwest::Error),
    #[error("viewer token upstream error: {0}")]
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
    subscription: Option<u64>,
    next_request_id: u64,
    requested_history: bool,
    diff_seq: AtomicU64,
}

impl ManagerViewerState {
    fn new() -> Self {
        Self {
            grid: TerminalGrid::with_history_limit(1, 1, 1024),
            cursor: None,
            subscription: None,
            next_request_id: 1,
            requested_history: false,
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
        info!(
            target = "private_beach",
            cols, history_rows, base_row, viewport_rows, "manager viewer grid reset"
        );
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
                let style_id = StyleId(*id);
                self.grid.style_table.insert_at(style_id, style);
                debug!(
                    target = "private_beach",
                    style_id = *id,
                    styles_registered = self.grid.style_table.len(),
                    "manager viewer applied style update"
                );
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
            WireHostFrame::Hello { subscription, .. } => {
                self.cursor = None;
                self.diff_seq.store(0, Ordering::SeqCst);
                self.subscription = Some(*subscription);
                self.requested_history = false;
                None
            }
            WireHostFrame::Grid {
                cols,
                history_rows,
                base_row,
                viewport_rows,
            } => {
                self.reset_grid(*cols, *history_rows, *base_row, *viewport_rows);
                self.requested_history = false;
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
        if let (Some(rows), Some(base_row)) = (frame.rows, frame.base_row) {
            debug!(
                target = "private_beach",
                rows, base_row, "manager viewer diff captured"
            );
        }
        let sequence = self.diff_seq.fetch_add(1, Ordering::SeqCst) + 1;
        StateDiff {
            sequence,
            emitted_at: SystemTime::now(),
            payload: build_terminal_payload(&frame),
        }
    }

    fn take_history_request(&mut self, history_rows: u32, base_row: u64) -> Option<ClientFrame> {
        if self.requested_history {
            return None;
        }
        let subscription = self.subscription?;
        let total_rows = base_row.saturating_add(history_rows as u64);
        if total_rows == 0 {
            return None;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.requested_history = true;
        Some(ClientFrame::RequestBackfill {
            subscription,
            request_id,
            start_row: 0,
            count: total_rows
                .min(u32::MAX as u64)
                .try_into()
                .unwrap_or(u32::MAX),
        })
    }
}

fn capture_terminal_frame_simple(
    grid: &TerminalGrid,
    cursor: Option<&ViewerCursor>,
) -> TerminalFrame {
    let col_count = grid.cols().max(1);
    let start_row = grid.first_row_id().unwrap_or_else(|| grid.row_offset());
    let end_row = grid.last_row_id().unwrap_or(start_row.saturating_sub(1));
    let rows = if end_row >= start_row {
        (end_row - start_row + 1) as usize
    } else {
        0
    };
    let base_row = start_row;
    let style_table = grid.style_table.clone();
    let style_entries = style_table.entries();
    if let Some((first_id, first_style)) = style_entries.get(0) {
        debug!(
            target = "private_beach",
            styles = style_entries.len(),
            sample_id = first_id.0,
            sample_fg = first_style.fg,
            sample_bg = first_style.bg,
            sample_attrs = first_style.attrs,
            "manager viewer style snapshot"
        );
    } else {
        debug!(
            target = "private_beach",
            styles = 0,
            "manager viewer style snapshot"
        );
    }
    let mut style_lookup = HashMap::with_capacity(style_entries.len());
    for (id, style) in &style_entries {
        style_lookup.insert(id.0, *style);
    }
    let mut lines = Vec::with_capacity(rows);
    let mut styled_lines = Vec::with_capacity(rows);

    if rows > 0 {
        for absolute in start_row..=end_row {
            let Some(index) = grid.index_of_row(absolute) else {
                continue;
            };
            let mut cells = Vec::with_capacity(col_count);
            for col in 0..col_count {
                let (raw_char, style_id) = grid
                    .get_cell_relaxed(index, col)
                    .map(|snapshot| unpack_cell(snapshot.cell))
                    .unwrap_or((' ', StyleId::DEFAULT));
                let ch = if raw_char == '\0' { ' ' } else { raw_char };
                let style = style_lookup.get(&style_id.0).copied().unwrap_or_default();
                cells.push(StyledCell {
                    ch,
                    style: CellStylePayload {
                        id: style_id.0,
                        fg: style.fg,
                        bg: style.bg,
                        attrs: style.attrs as u32,
                    },
                });
            }
            while let Some(last) = cells.last() {
                if last.ch == ' ' && last.style.id == StyleId::DEFAULT.0 {
                    cells.pop();
                } else {
                    break;
                }
            }
            let line: String = cells.iter().map(|cell| cell.ch).collect();
            lines.push(line);
            styled_lines.push(cells);
        }
    }

    let cursor = cursor.map(|cursor| CursorPosition {
        row: cursor.abs_row,
        col: cursor.col,
    });

    let styles = style_entries
        .into_iter()
        .map(|(id, style)| StyleDefinition {
            id: id.0,
            fg: style.fg,
            bg: style.bg,
            attrs: style.attrs as u32,
        })
        .collect();

    TerminalFrame {
        lines,
        styled_lines: Some(styled_lines),
        styles: Some(styles),
        cols: Some(col_count),
        rows: Some(rows),
        base_row: Some(base_row),
        cursor,
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
            serde_json::json!({
                "row": cursor.row,
                "col": cursor.col,
            }),
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
    if let Some(base_row) = frame.base_row {
        payload.insert("base_row".into(), serde_json::json!(base_row));
    }
    serde_json::Value::Object(payload)
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("session not found")]
    SessionNotFound,
    #[error("controller token mismatch")]
    ControllerMismatch,
    #[error("controller lease required")]
    ControllerLeaseRequired,
    #[error("controller pairing not found")]
    ControllerPairingNotFound,
    #[error("sessions must belong to the same private beach")]
    CrossBeachPairing,
    #[error("private beach not found")]
    PrivateBeachNotFound,
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("invalid layout: {0}")]
    InvalidLayout(String),
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
    PairingAdded,
    PairingRemoved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "controller_update_cadence", rename_all = "snake_case")]
pub enum ControllerUpdateCadence {
    Fast,
    Balanced,
    Slow,
}

impl Default for ControllerUpdateCadence {
    fn default() -> Self {
        ControllerUpdateCadence::Balanced
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingTransportKind {
    FastPath,
    HttpFallback,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairingTransportStatus {
    pub transport: PairingTransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl PairingTransportStatus {
    pub fn pending() -> Self {
        Self {
            transport: PairingTransportKind::Pending,
            last_event_ms: None,
            latency_ms: None,
            last_error: None,
        }
    }

    pub fn fast_path(now_ms: i64) -> Self {
        Self {
            transport: PairingTransportKind::FastPath,
            last_event_ms: Some(now_ms),
            latency_ms: None,
            last_error: None,
        }
    }

    pub fn http_fallback(now_ms: i64, last_error: Option<String>) -> Self {
        Self {
            transport: PairingTransportKind::HttpFallback,
            last_event_ms: Some(now_ms),
            latency_ms: None,
            last_error,
        }
    }

    pub fn with_latency(mut self, latency_ms: Option<u64>) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    pub fn with_error(mut self, last_error: Option<String>) -> Self {
        self.last_error = last_error;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerPairing {
    pub pairing_id: String,
    pub private_beach_id: String,
    pub controller_session_id: String,
    pub child_session_id: String,
    pub prompt_template: Option<String>,
    pub update_cadence: ControllerUpdateCadence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_status: Option<PairingTransportStatus>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPairingAction {
    Added,
    Updated,
    Removed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControllerPairingEvent {
    pub controller_session_id: String,
    pub child_session_id: String,
    pub action: ControllerPairingAction,
    pub pairing: Option<ControllerPairing>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum StreamEvent {
    ControllerEvent(ControllerEvent),
    State(StateDiff),
    Health(HealthHeartbeat),
    ControllerPairing(ControllerPairingEvent),
}

impl StreamEvent {
    pub fn as_named_json(&self) -> (&'static str, Option<String>) {
        match self {
            StreamEvent::ControllerEvent(ev) => {
                ("controller_event", serde_json::to_string(ev).ok())
            }
            StreamEvent::State(diff) => ("state", serde_json::to_string(diff).ok()),
            StreamEvent::Health(hb) => ("health", serde_json::to_string(hb).ok()),
            StreamEvent::ControllerPairing(event) => {
                ("controller_pairing", serde_json::to_string(event).ok())
            }
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

#[derive(Debug, FromRow)]
struct ControllerPairingRow {
    controller_session_id: Uuid,
    child_session_id: Uuid,
    controller_origin_session_id: Uuid,
    child_origin_session_id: Uuid,
    prompt_template: Option<String>,
    update_cadence: ControllerUpdateCadence,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct ControllerAgentSessionRow {
    controller_origin_session_id: Uuid,
    controller_metadata: Option<Json<serde_json::Value>>,
}

fn metadata_role_is_agent(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("role"))
        .and_then(|role| role.as_str())
        .map(|role| role.eq_ignore_ascii_case("agent"))
        .unwrap_or(false)
}

impl ControllerPairingRow {
    fn into_pairing(self, private_beach_id: &Uuid) -> ControllerPairing {
        ControllerPairing {
            pairing_id: format!(
                "{}:{}",
                self.controller_origin_session_id, self.child_origin_session_id
            ),
            private_beach_id: private_beach_id.to_string(),
            controller_session_id: self.controller_origin_session_id.to_string(),
            child_session_id: self.child_origin_session_id.to_string(),
            prompt_template: self.prompt_template,
            update_cadence: self.update_cadence,
            transport_status: None,
            created_at_ms: self.created_at.timestamp_millis(),
            updated_at_ms: self.updated_at.timestamp_millis(),
        }
    }
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

    pub(crate) async fn viewer_token(
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
        arc.spawn_receivers(self.clone());
        self.fast_paths.insert(session_id, arc).await;
    }

    pub async fn fast_path_for(&self, session_id: &str) -> Option<Arc<FastPathSession>> {
        self.fast_paths.get(session_id).await
    }

    pub async fn session_metrics_labels(&self, session_id: &str) -> Option<(String, String)> {
        {
            let sessions = self.fallback.sessions.read().await;
            if let Some(record) = sessions.get(session_id) {
                return Some((record.private_beach_id.clone(), session_id.to_string()));
            }
        }

        match &self.backend {
            Backend::Postgres(pool) => {
                if let Ok(session_uuid) = Uuid::parse_str(session_id) {
                    if let Ok(identifiers) =
                        self.fetch_session_identifiers(pool, &session_uuid).await
                    {
                        return Some((
                            identifiers.private_beach_id.to_string(),
                            session_uuid.to_string(),
                        ));
                    }
                }
            }
            Backend::Memory => {}
        }
        None
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
                StreamEvent::ControllerPairing(_) => "controller_pairing",
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

    async fn publish_pairing_event(
        &self,
        controller_session_id: &str,
        child_session_id: &str,
        action: ControllerPairingAction,
        pairing: Option<ControllerPairing>,
    ) {
        self.publish(
            controller_session_id,
            StreamEvent::ControllerPairing(ControllerPairingEvent {
                controller_session_id: controller_session_id.to_string(),
                child_session_id: child_session_id.to_string(),
                action,
                pairing,
            }),
        )
        .await;
    }

    async fn update_pairing_transport_status(
        &self,
        child_session_id: &str,
        status: PairingTransportStatus,
    ) {
        let controllers = self.fallback.controllers_for_child(child_session_id).await;
        if controllers.is_empty() {
            return;
        }

        for controller_session_id in controllers {
            if let Some(updated) = self
                .fallback
                .update_pairing_status(&controller_session_id, child_session_id, status.clone())
                .await
            {
                metrics::CONTROLLER_PAIRINGS_EVENTS
                    .with_label_values(&[
                        updated.private_beach_id.as_str(),
                        controller_session_id.as_str(),
                        pairing_action_label(&ControllerPairingAction::Updated),
                    ])
                    .inc();
                self.publish_pairing_event(
                    &controller_session_id,
                    child_session_id,
                    ControllerPairingAction::Updated,
                    Some(updated),
                )
                .await;
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
        viewer_token: Option<String>,
        mcp: bool,
    ) -> Result<(StatusCode, JoinSessionResponsePayload), JoinForwardError> {
        let url = format!(
            "{}/sessions/{}/join",
            self.road_base_url.trim_end_matches('/'),
            session_id
        );
        let body = serde_json::json!({
            "passphrase": passphrase,
            "viewer_token": viewer_token,
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

    pub async fn list_controller_pairings(
        &self,
        controller_session_id: &str,
    ) -> Result<Vec<ControllerPairing>, StateError> {
        // Reads are allowed even when no controller lease is active so that viewer dashboards
        // can inspect pairing assignments without being connected as a controller.
        match &self.backend {
            Backend::Memory => {
                let record = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions.get(controller_session_id).cloned()
                }
                .ok_or(StateError::SessionNotFound)?;

                let pairings = self.fallback.list_pairings(controller_session_id).await;
                let label0 = record.private_beach_id.clone();
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[label0.as_str(), controller_session_id])
                    .set(pairings.len() as i64);
                Ok(pairings)
            }
            Backend::Postgres(pool) => {
                let controller_uuid = parse_uuid(controller_session_id, "controller_session_id")?;
                let identifiers = self
                    .fetch_session_identifiers(pool, &controller_uuid)
                    .await?;

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                let rows: Vec<ControllerPairingRow> = sqlx::query_as(
                    r#"
                    SELECT
                        cp.controller_session_id,
                        cp.child_session_id,
                        controller.origin_session_id AS controller_origin_session_id,
                        child.origin_session_id AS child_origin_session_id,
                        cp.prompt_template,
                        cp.update_cadence,
                        cp.created_at,
                        cp.updated_at
                    FROM controller_pairing cp
                    INNER JOIN session controller ON controller.id = cp.controller_session_id
                    INNER JOIN session child ON child.id = cp.child_session_id
                    WHERE cp.controller_session_id = $1
                    ORDER BY cp.created_at ASC
                    "#,
                )
                .bind(identifiers.session_id)
                .fetch_all(tx.as_mut())
                .await?;
                tx.commit().await?;

                let private_beach_id = identifiers.private_beach_id;
                let label0 = private_beach_id.to_string();
                let pairings: Vec<ControllerPairing> = rows
                    .into_iter()
                    .map(|row| row.into_pairing(&private_beach_id))
                    .collect();
                let pairings = self
                    .fallback
                    .set_pairings(controller_session_id, pairings)
                    .await;
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[label0.as_str(), controller_session_id])
                    .set(pairings.len() as i64);
                Ok(pairings)
            }
        }
    }

    pub async fn upsert_controller_pairing(
        &self,
        controller_session_id: &str,
        child_session_id: &str,
        prompt_template: Option<String>,
        update_cadence: Option<ControllerUpdateCadence>,
        actor_account_id: Option<Uuid>,
    ) -> Result<ControllerPairing, StateError> {
        match &self.backend {
            Backend::Memory => {
                let (controller, child) = {
                    let sessions = self.fallback.sessions.read().await;
                    let Some(controller) = sessions.get(controller_session_id).cloned() else {
                        return Err(StateError::SessionNotFound);
                    };
                    let Some(child) = sessions.get(child_session_id).cloned() else {
                        return Err(StateError::SessionNotFound);
                    };
                    (controller, child)
                };

                if controller.controller_token.is_none() {
                    return Err(StateError::ControllerLeaseRequired);
                }
                if controller.private_beach_id != child.private_beach_id {
                    return Err(StateError::CrossBeachPairing);
                }

                let existing = self.fallback.list_pairings(controller_session_id).await;
                let action = if existing
                    .iter()
                    .any(|p| p.child_session_id == child_session_id)
                {
                    ControllerPairingAction::Updated
                } else {
                    ControllerPairingAction::Added
                };

                let now = now_ms();
                let pairing = ControllerPairing {
                    pairing_id: format!("{controller_session_id}:{child_session_id}"),
                    private_beach_id: controller.private_beach_id.clone(),
                    controller_session_id: controller_session_id.to_string(),
                    child_session_id: child_session_id.to_string(),
                    prompt_template: prompt_template.clone(),
                    update_cadence: update_cadence.unwrap_or_default(),
                    transport_status: None,
                    created_at_ms: now,
                    updated_at_ms: now,
                };

                let pairing = self.fallback.upsert_pairing(pairing).await;
                {
                    let mut sessions = self.fallback.sessions.write().await;
                    if let Some(record) = sessions.get_mut(controller_session_id) {
                        record.append_event(ControllerEventType::PairingAdded, prompt_template);
                    }
                }

                let count = self.fallback.pairing_count(controller_session_id).await as i64;
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[
                        controller.private_beach_id.as_str(),
                        controller_session_id,
                    ])
                    .set(count);
                metrics::CONTROLLER_PAIRINGS_EVENTS
                    .with_label_values(&[
                        controller.private_beach_id.as_str(),
                        controller_session_id,
                        pairing_action_label(&action),
                    ])
                    .inc();

                self.publish_pairing_event(
                    controller_session_id,
                    child_session_id,
                    action,
                    Some(pairing.clone()),
                )
                .await;

                Ok(pairing)
            }
            Backend::Postgres(pool) => {
                let controller_uuid = parse_uuid(controller_session_id, "controller_session_id")?;
                let child_uuid = parse_uuid(child_session_id, "child_session_id")?;
                let controller_identifiers = self
                    .fetch_session_identifiers(pool, &controller_uuid)
                    .await?;
                let child_identifiers = self.fetch_session_identifiers(pool, &child_uuid).await?;

                if controller_identifiers.private_beach_id != child_identifiers.private_beach_id {
                    return Err(StateError::CrossBeachPairing);
                }

                let lease = match self
                    .fetch_active_lease(pool, controller_identifiers.session_id)
                    .await
                {
                    Ok(row) => row,
                    Err(StateError::ControllerMismatch) => {
                        return Err(StateError::ControllerLeaseRequired);
                    }
                    Err(err) => return Err(err),
                };
                if !is_active_lease(lease.expires_at, lease.revoked_at) {
                    return Err(StateError::ControllerLeaseRequired);
                }

                let cadence = update_cadence.unwrap_or_default();
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &controller_identifiers.private_beach_id)
                    .await?;
                self.set_account_context_tx(&mut tx, actor_account_id.as_ref())
                    .await?;

                let prompt_clone = prompt_template.clone();
                let row: ControllerPairingRow = sqlx::query_as(
                    r#"
                    INSERT INTO controller_pairing (
                        controller_session_id,
                        child_session_id,
                        prompt_template,
                        update_cadence
                    )
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (controller_session_id, child_session_id)
                    DO UPDATE SET
                        prompt_template = EXCLUDED.prompt_template,
                        update_cadence = EXCLUDED.update_cadence,
                        updated_at = NOW()
                    RETURNING
                        controller_session_id,
                        child_session_id,
                        (SELECT origin_session_id FROM session WHERE id = controller_pairing.controller_session_id) AS controller_origin_session_id,
                        (SELECT origin_session_id FROM session WHERE id = controller_pairing.child_session_id) AS child_origin_session_id,
                        prompt_template,
                        update_cadence,
                        created_at,
                        updated_at
                    "#,
                )
                .bind(controller_identifiers.session_id)
                .bind(child_identifiers.session_id)
                .bind(prompt_template)
                .bind(cadence)
                .fetch_one(tx.as_mut())
                .await?;

                let action = if row.created_at == row.updated_at {
                    ControllerPairingAction::Added
                } else {
                    ControllerPairingAction::Updated
                };

                self.insert_controller_event(
                    &mut tx,
                    controller_identifiers.session_id,
                    "pairing_added",
                    Some(lease.id),
                    lease.controller_account_id,
                    actor_account_id,
                    prompt_clone,
                )
                .await?;

                let count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)::bigint
                    FROM controller_pairing
                    WHERE controller_session_id = $1
                    "#,
                )
                .bind(controller_identifiers.session_id)
                .fetch_one(tx.as_mut())
                .await?;

                tx.commit().await?;

                let private_beach = controller_identifiers.private_beach_id.to_string();
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[private_beach.as_str(), controller_session_id])
                    .set(count);
                metrics::CONTROLLER_PAIRINGS_EVENTS
                    .with_label_values(&[
                        private_beach.as_str(),
                        controller_session_id,
                        pairing_action_label(&action),
                    ])
                    .inc();

                let pairing = row.into_pairing(&controller_identifiers.private_beach_id);
                let pairing = self.fallback.upsert_pairing(pairing).await;
                self.publish_pairing_event(
                    controller_session_id,
                    child_session_id,
                    action,
                    Some(pairing.clone()),
                )
                .await;
                Ok(pairing)
            }
        }
    }

    pub async fn delete_controller_pairing(
        &self,
        controller_session_id: &str,
        child_session_id: &str,
        actor_account_id: Option<Uuid>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                let controller = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions.get(controller_session_id).cloned()
                }
                .ok_or(StateError::SessionNotFound)?;

                if controller.controller_token.is_none() {
                    return Err(StateError::ControllerLeaseRequired);
                }

                let removed = self
                    .fallback
                    .remove_pairing(controller_session_id, child_session_id)
                    .await;
                if !removed {
                    return Err(StateError::ControllerPairingNotFound);
                }

                {
                    let mut sessions = self.fallback.sessions.write().await;
                    if let Some(record) = sessions.get_mut(controller_session_id) {
                        record.append_event(ControllerEventType::PairingRemoved, None);
                    }
                }

                let count = self.fallback.pairing_count(controller_session_id).await as i64;
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[
                        controller.private_beach_id.as_str(),
                        controller_session_id,
                    ])
                    .set(count);
                metrics::CONTROLLER_PAIRINGS_EVENTS
                    .with_label_values(&[
                        controller.private_beach_id.as_str(),
                        controller_session_id,
                        pairing_action_label(&ControllerPairingAction::Removed),
                    ])
                    .inc();

                self.publish_pairing_event(
                    controller_session_id,
                    child_session_id,
                    ControllerPairingAction::Removed,
                    None,
                )
                .await;
                Ok(())
            }
            Backend::Postgres(pool) => {
                let controller_uuid = parse_uuid(controller_session_id, "controller_session_id")?;
                let child_uuid = parse_uuid(child_session_id, "child_session_id")?;
                let controller_identifiers = self
                    .fetch_session_identifiers(pool, &controller_uuid)
                    .await?;
                let child_identifiers = self.fetch_session_identifiers(pool, &child_uuid).await?;

                if controller_identifiers.private_beach_id != child_identifiers.private_beach_id {
                    return Err(StateError::CrossBeachPairing);
                }

                let lease = match self
                    .fetch_active_lease(pool, controller_identifiers.session_id)
                    .await
                {
                    Ok(row) => row,
                    Err(StateError::ControllerMismatch) => {
                        return Err(StateError::ControllerLeaseRequired);
                    }
                    Err(err) => return Err(err),
                };
                if !is_active_lease(lease.expires_at, lease.revoked_at) {
                    return Err(StateError::ControllerLeaseRequired);
                }

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &controller_identifiers.private_beach_id)
                    .await?;
                self.set_account_context_tx(&mut tx, actor_account_id.as_ref())
                    .await?;

                let row: Option<ControllerPairingRow> = sqlx::query_as(
                    r#"
                    DELETE FROM controller_pairing
                    WHERE controller_session_id = $1
                      AND child_session_id = $2
                    RETURNING
                        controller_session_id,
                        child_session_id,
                        (SELECT origin_session_id FROM session WHERE id = controller_pairing.controller_session_id) AS controller_origin_session_id,
                        (SELECT origin_session_id FROM session WHERE id = controller_pairing.child_session_id) AS child_origin_session_id,
                        prompt_template,
                        update_cadence,
                        created_at,
                        updated_at
                    "#,
                )
                .bind(controller_identifiers.session_id)
                .bind(child_identifiers.session_id)
                .fetch_optional(tx.as_mut())
                .await?;

                let Some(row) = row else {
                    tx.rollback().await.ok();
                    return Err(StateError::ControllerPairingNotFound);
                };

                self.insert_controller_event(
                    &mut tx,
                    controller_identifiers.session_id,
                    "pairing_removed",
                    Some(lease.id),
                    lease.controller_account_id,
                    actor_account_id,
                    None,
                )
                .await?;

                let count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)::bigint
                    FROM controller_pairing
                    WHERE controller_session_id = $1
                    "#,
                )
                .bind(controller_identifiers.session_id)
                .fetch_one(tx.as_mut())
                .await?;

                tx.commit().await?;

                let pairing = row.into_pairing(&controller_identifiers.private_beach_id);
                let private_beach = controller_identifiers.private_beach_id.to_string();
                metrics::CONTROLLER_PAIRINGS_ACTIVE
                    .with_label_values(&[private_beach.as_str(), controller_session_id])
                    .set(count);
                metrics::CONTROLLER_PAIRINGS_EVENTS
                    .with_label_values(&[
                        private_beach.as_str(),
                        controller_session_id,
                        pairing_action_label(&ControllerPairingAction::Removed),
                    ])
                    .inc();

                self.fallback
                    .remove_pairing(controller_session_id, child_session_id)
                    .await;

                self.publish_pairing_event(
                    controller_session_id,
                    child_session_id,
                    ControllerPairingAction::Removed,
                    Some(pairing),
                )
                .await;
                Ok(())
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

                let session_uuid_str = session_uuid.to_string();
                let trace_context = self
                    .build_agent_trace_context(pool, &identifiers, &actions)
                    .await;
                let mut fast_path_error: Option<String> = None;
                match send_actions_over_fast_path(&self.fast_paths, &session_uuid_str, &actions)
                    .await
                {
                    Ok(true) => {
                        let now = now_ms();
                        self.update_pairing_transport_status(
                            &session_uuid_str,
                            PairingTransportStatus::fast_path(now),
                        )
                        .await;

                        let label0 = identifiers.private_beach_id.to_string();
                        let label1 = session_uuid_str.clone();
                        metrics::FASTPATH_ACTIONS_SENT
                            .with_label_values(&[label0.as_str(), label1.as_str()])
                            .inc_by(actions.len() as u64);

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
                                timestamp_ms: now,
                                reason: None,
                                controller_account_id: lease
                                    .controller_account_id
                                    .map(|u| u.to_string()),
                                issued_by_account_id: actor_account_id.map(|u| u.to_string()),
                            }),
                        )
                        .await;
                        if let Some((agent_sessions, payload)) = trace_context.as_ref() {
                            Self::log_agent_bridge_payload(
                                agent_sessions,
                                &session_uuid,
                                &identifiers.private_beach_id,
                                payload,
                                "agent_to_child",
                                "fast_path",
                            );
                        }
                        return Ok(());
                    }
                    Ok(false) => {
                        let now = now_ms();
                        self.update_pairing_transport_status(
                            &session_uuid_str,
                            PairingTransportStatus::http_fallback(now, None),
                        )
                        .await;
                    }
                    Err(err) => {
                        warn!(
                            target = "fast_path",
                            session_id = %session_uuid,
                            error = %err,
                            "fast-path send failed; falling back to HTTP transport"
                        );
                        fast_path_error = Some(err.to_string());
                        let now = now_ms();
                        self.update_pairing_transport_status(
                            &session_uuid_str,
                            PairingTransportStatus::http_fallback(now, Some(err.to_string())),
                        )
                        .await;
                    }
                }

                self.enqueue_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid_str,
                    actions.clone(),
                )
                .await?;

                // Metrics: enqueue count and queue depth gauge
                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid_str.clone();
                let labels = [label0.as_str(), label1.as_str()];
                metrics::ACTIONS_ENQUEUED
                    .with_label_values(&labels)
                    .inc_by(actions.len() as u64);
                let depth = self
                    .pending_actions_count(
                        &identifiers.private_beach_id.to_string(),
                        &session_uuid_str,
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
                        &session_uuid_str,
                    )
                    .await
                    .unwrap_or(0);
                metrics::QUEUE_LAG
                    .with_label_values(&labels)
                    .set(pending as i64);
                metrics::FASTPATH_ACTIONS_FALLBACK
                    .with_label_values(&labels)
                    .inc_by(actions.len() as u64);

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

                if let Some(error) = fast_path_error {
                    debug!(
                        target = "controller_pairing",
                        session_id = %session_uuid,
                        error = %error,
                        "fast-path unavailable; actions queued via fallback"
                    );
                }

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

                if let Some((agent_sessions, payload)) = trace_context.as_ref() {
                    Self::log_agent_bridge_payload(
                        agent_sessions,
                        &session_uuid,
                        &identifiers.private_beach_id,
                        payload,
                        "agent_to_child",
                        "http_fallback",
                    );
                }

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

                let trace_context = self
                    .build_agent_trace_context(pool, &identifiers, &actions)
                    .await;

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

                if let Some((agent_sessions, payload)) = trace_context.as_ref() {
                    Self::log_agent_bridge_payload(
                        agent_sessions,
                        &session_uuid,
                        &identifiers.private_beach_id,
                        payload,
                        "agent_to_child",
                        "delivery",
                    );
                }

                Ok(actions)
            }
        }
    }

    pub async fn ack_actions(
        &self,
        session_id: &str,
        acks: Vec<ActionAck>,
        _actor_account_id: Option<Uuid>,
        via_fast_path: bool,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                self.fallback.remove_actions(session_id, &acks).await;
                if !acks.is_empty() {
                    let event_time = now_ms();
                    let latency = acks
                        .iter()
                        .find_map(|ack| ack.latency_ms)
                        .map(|ms| ms as u64);
                    let error_message = acks
                        .iter()
                        .find(|ack| !matches!(ack.status, AckStatus::Ok))
                        .and_then(|ack| {
                            ack.error_message.clone().or_else(|| ack.error_code.clone())
                        });
                    let mut status = if via_fast_path {
                        PairingTransportStatus::fast_path(event_time)
                    } else {
                        PairingTransportStatus::http_fallback(event_time, None)
                    };
                    status = status.with_latency(latency);
                    if let Some(err) = error_message {
                        status = status.with_error(Some(err));
                    }
                    self.update_pairing_transport_status(session_id, status)
                        .await;
                }
                Ok(())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let session_uuid_str = session_uuid.to_string();
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                self.ack_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid_str,
                    &acks,
                )
                .await?;
                // Metrics: record latencies for successful acks
                let label0 = identifiers.private_beach_id.to_string();
                let label1 = session_uuid_str.clone();
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
                let label1 = session_uuid_str.clone();
                let pending = self
                    .pending_actions_pending_count(&label0, &session_uuid_str)
                    .await
                    .unwrap_or(0);
                metrics::QUEUE_LAG
                    .with_label_values(&[label0.as_str(), label1.as_str()])
                    .set(pending as i64);
                if via_fast_path {
                    metrics::FASTPATH_ACKS_RECEIVED
                        .with_label_values(&[label0.as_str(), label1.as_str()])
                        .inc_by(acks.len() as u64);
                }
                self.fallback.remove_actions(session_id, &acks).await;
                if !acks.is_empty() {
                    let event_time = now_ms();
                    let latency = acks
                        .iter()
                        .find_map(|ack| ack.latency_ms)
                        .map(|ms| ms as u64);
                    let error_message = acks
                        .iter()
                        .find(|ack| !matches!(ack.status, AckStatus::Ok))
                        .and_then(|ack| {
                            ack.error_message.clone().or_else(|| ack.error_code.clone())
                        });
                    let mut status = if via_fast_path {
                        PairingTransportStatus::fast_path(event_time)
                    } else {
                        PairingTransportStatus::http_fallback(event_time, None)
                    };
                    status = status.with_latency(latency);
                    if let Some(err) = error_message {
                        status = status.with_error(Some(err));
                    }
                    self.update_pairing_transport_status(&session_uuid_str, status)
                        .await;
                }
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

    pub async fn record_state(
        &self,
        session_id: &str,
        diff: StateDiff,
        via_fast_path: bool,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => self.record_state_memory(session_id, diff).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                debug!(
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
                if via_fast_path {
                    metrics::FASTPATH_STATE_RECEIVED
                        .with_label_values(&[label0.as_str(), label1.as_str()])
                        .inc();
                }
                Ok(())
            }
        }
    }

    pub async fn state_snapshot(&self, session_id: &str) -> Result<Option<StateDiff>, StateError> {
        match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                Ok(sessions
                    .get(session_id)
                    .and_then(|record| record.last_state.clone()))
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let private_beach_id = identifiers.private_beach_id.to_string();
                let session_uuid_str = session_uuid.to_string();
                if let Some(snapshot) = self
                    .load_state_redis(&private_beach_id, &session_uuid_str)
                    .await?
                {
                    return Ok(Some(snapshot));
                }
                let fallback_state = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions
                        .get(&session_uuid_str)
                        .and_then(|record| record.last_state.clone())
                };
                Ok(fallback_state)
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
                let identifiers = match self
                    .fetch_session_identifiers_for_private_beach(pool, &session_uuid, &beach_uuid)
                    .await?
                {
                    Some(found) => found,
                    None => match self.fetch_session_identifiers(pool, &session_uuid).await {
                        Ok(_) => return Err(StateError::PrivateBeachNotFound),
                        Err(StateError::SessionNotFound) => {
                            return Err(StateError::SessionNotFound);
                        }
                        Err(other) => return Err(other),
                    },
                };

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
        let (private_beach_id, join_code) = {
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
                join_code,
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

fn legacy_layout_to_canvas(
    layout_value: Option<&serde_json::Value>,
    now_ms: i64,
) -> crate::routes::CanvasLayout {
    let mut tiles = HashMap::new();
    if let Some(items) = layout_value
        .and_then(|value| value.get("layout"))
        .and_then(|value| value.as_array())
    {
        for item in items {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let Some(id) = id else {
                continue;
            };
            let grid_x = item
                .get("x")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .max(0.0);
            let grid_y = item
                .get("y")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .max(0.0);
            let grid_w = item
                .get("w")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0)
                .max(1.0);
            let grid_h = item
                .get("h")
                .and_then(|v| v.as_f64())
                .unwrap_or(2.0)
                .max(1.0);
            let width_px = item
                .get("widthPx")
                .and_then(|v| v.as_f64())
                .map(|v| v.max(50.0))
                .unwrap_or_else(|| grid_w * 320.0);
            let height_px = item
                .get("heightPx")
                .and_then(|v| v.as_f64())
                .map(|v| v.max(50.0))
                .unwrap_or_else(|| grid_h * 240.0);
            let zoom = item.get("zoom").and_then(|v| v.as_f64());
            let locked = item.get("locked").and_then(|v| v.as_bool());
            let toolbar_pinned = item.get("toolbarPinned").and_then(|v| v.as_bool());

            tiles.insert(
                id.clone(),
                crate::routes::CanvasTileNode {
                    id,
                    position: crate::routes::CanvasPoint {
                        x: (grid_x * 320.0).round(),
                        y: (grid_y * 240.0).round(),
                    },
                    size: crate::routes::CanvasSize {
                        width: width_px.round(),
                        height: height_px.round(),
                    },
                    z_index: 1,
                    group_id: None,
                    zoom,
                    locked,
                    toolbar_pinned,
                    metadata: None,
                },
            );
        }
    }

    crate::routes::CanvasLayout {
        version: 3,
        viewport: crate::routes::CanvasViewport::default(),
        tiles,
        agents: HashMap::new(),
        groups: HashMap::new(),
        control_assignments: HashMap::new(),
        metadata: crate::routes::CanvasMetadata {
            created_at: now_ms,
            updated_at: now_ms,
            migrated_from: Some(2),
            agent_relationships: HashMap::new(),
            agent_relationship_order: Vec::new(),
        },
    }
    .ensure_version()
    .unwrap_or_else(|_| crate::routes::CanvasLayout::empty(now_ms))
}

impl InnerState {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            pairings: RwLock::new(HashMap::new()),
            canvas_layouts: RwLock::new(HashMap::new()),
        }
    }

    async fn get_canvas_layout(&self, beach_id: &str, now_ms: i64) -> crate::routes::CanvasLayout {
        let layouts = self.canvas_layouts.read().await;
        layouts
            .get(beach_id)
            .cloned()
            .unwrap_or_else(|| crate::routes::CanvasLayout::empty(now_ms))
    }

    async fn set_canvas_layout(
        &self,
        beach_id: impl Into<String>,
        layout: crate::routes::CanvasLayout,
    ) {
        let mut layouts = self.canvas_layouts.write().await;
        layouts.insert(beach_id.into(), layout);
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

    async fn upsert_pairing(&self, mut pairing: ControllerPairing) -> ControllerPairing {
        let mut pairings = self.pairings.write().await;
        let entry = pairings
            .entry(pairing.controller_session_id.clone())
            .or_default();
        if let Some(existing) = entry
            .iter_mut()
            .find(|p| p.child_session_id == pairing.child_session_id)
        {
            if pairing.transport_status.is_none() {
                pairing.transport_status = existing.transport_status.clone();
            }
            *existing = pairing.clone();
            pairing
        } else {
            if pairing.transport_status.is_none() {
                pairing.transport_status = Some(PairingTransportStatus::pending());
            }
            entry.push(pairing.clone());
            pairing
        }
    }

    async fn remove_pairing(&self, controller_session_id: &str, child_session_id: &str) -> bool {
        let mut pairings = self.pairings.write().await;
        let mut remove_entry = false;
        let mut removed = false;
        if let Some(list) = pairings.get_mut(controller_session_id) {
            let before = list.len();
            list.retain(|pairing| pairing.child_session_id != child_session_id);
            removed = before != list.len();
            if list.is_empty() {
                remove_entry = true;
            }
        }
        if remove_entry {
            pairings.remove(controller_session_id);
        }
        removed
    }

    async fn list_pairings(&self, controller_session_id: &str) -> Vec<ControllerPairing> {
        self.pairings
            .read()
            .await
            .get(controller_session_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn pairing_count(&self, controller_session_id: &str) -> usize {
        self.pairings
            .read()
            .await
            .get(controller_session_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    async fn set_pairings(
        &self,
        controller_session_id: &str,
        mut pairings: Vec<ControllerPairing>,
    ) -> Vec<ControllerPairing> {
        let mut guard = self.pairings.write().await;
        if pairings.is_empty() {
            guard.remove(controller_session_id);
            return pairings;
        }

        if let Some(existing) = guard.get(controller_session_id) {
            let mut statuses = HashMap::new();
            for pairing in existing {
                statuses.insert(
                    pairing.child_session_id.clone(),
                    pairing.transport_status.clone(),
                );
            }
            for pairing in pairings.iter_mut() {
                if pairing.transport_status.is_none() {
                    if let Some(status) = statuses
                        .get(&pairing.child_session_id)
                        .and_then(|value| value.clone())
                    {
                        pairing.transport_status = Some(status);
                    } else {
                        pairing.transport_status = Some(PairingTransportStatus::pending());
                    }
                }
            }
        } else {
            for pairing in pairings.iter_mut() {
                if pairing.transport_status.is_none() {
                    pairing.transport_status = Some(PairingTransportStatus::pending());
                }
            }
        }

        guard.insert(controller_session_id.to_string(), pairings.clone());
        pairings
    }

    async fn controllers_for_child(&self, child_session_id: &str) -> Vec<String> {
        let guard = self.pairings.read().await;
        guard
            .iter()
            .filter_map(|(controller, list)| {
                if list
                    .iter()
                    .any(|pairing| pairing.child_session_id == child_session_id)
                {
                    Some(controller.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    async fn update_pairing_status(
        &self,
        controller_session_id: &str,
        child_session_id: &str,
        status: PairingTransportStatus,
    ) -> Option<ControllerPairing> {
        let mut guard = self.pairings.write().await;
        if let Some(list) = guard.get_mut(controller_session_id) {
            if let Some(pairing) = list
                .iter_mut()
                .find(|p| p.child_session_id == child_session_id)
            {
                if pairing.transport_status.as_ref() == Some(&status) {
                    return None;
                }
                pairing.transport_status = Some(status);
                return Some(pairing.clone());
            }
        }
        None
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
    hints.insert(
        "fast_path_webrtc".into(),
        serde_json::json!({
            "offer_path": format!("/fastpath/sessions/{session_id}/webrtc/offer"),
            "ice_path": format!("/fastpath/sessions/{session_id}/webrtc/ice"),
            "channels": {
                "actions": "mgr-actions",
                "acks": "mgr-acks",
                "state": "mgr-state"
            },
            "status": "experimental"
        }),
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
        "pairing_added" => ControllerEventType::PairingAdded,
        "pairing_removed" => ControllerEventType::PairingRemoved,
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

fn pairing_action_label(action: &ControllerPairingAction) -> &'static str {
    match action {
        ControllerPairingAction::Added => "added",
        ControllerPairingAction::Updated => "updated",
        ControllerPairingAction::Removed => "removed",
    }
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
        let controller_token = record.controller_token.clone();
        let event_time = now_ms();
        let event = StreamEvent::ControllerEvent(ControllerEvent {
            id: Uuid::new_v4().to_string(),
            event_type: ControllerEventType::ActionsQueued,
            controller_token,
            timestamp_ms: event_time,
            reason: None,
            controller_account_id: None,
            issued_by_account_id: None,
        });
        drop(sessions);

        self.update_pairing_transport_status(
            session_id,
            PairingTransportStatus::http_fallback(event_time, None),
        )
        .await;

        self.publish(session_id, event).await;
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

    async fn fetch_session_identifiers_for_private_beach(
        &self,
        pool: &PgPool,
        session_uuid: &Uuid,
        private_beach_uuid: &Uuid,
    ) -> Result<Option<DbSessionIdentifiers>, StateError> {
        let row = sqlx::query_as::<_, DbSessionIdentifiers>(
            r#"
            SELECT id AS session_id, private_beach_id
            FROM session
            WHERE origin_session_id = $1
              AND private_beach_id = $2
            "#,
        )
        .bind(session_uuid)
        .bind(private_beach_uuid)
        .fetch_optional(pool)
        .await?;

        Ok(row)
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

    async fn load_state_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<Option<StateDiff>, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_state_key(private_beach_id, session_id);
            let payload: Option<String> =
                redis::cmd("GET").arg(&key).query_async(&mut conn).await?;
            if let Some(raw) = payload {
                let diff = serde_json::from_str(&raw)?;
                return Ok(Some(diff));
            }
        }
        Ok(None)
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
    ) -> Result<crate::routes::CanvasLayout, StateError> {
        let now_ms = Utc::now().timestamp_millis();
        match &self.backend {
            Backend::Memory => Ok(self.fallback.get_canvas_layout(id_str, now_ms).await),
            Backend::Postgres(pool) => {
                let meta = self.get_private_beach(id_str, account).await?;
                let id = parse_uuid(id_str, "id")?;
                let mut tx = pool.begin().await?;
                if let Some(a) = account {
                    self.set_account_context_tx(&mut tx, Some(&a)).await?;
                } else {
                    self.set_account_context_tx(&mut tx, None).await?;
                }
                self.set_rls_context_tx(&mut tx, &id).await?;
                let row: Option<Json<serde_json::Value>> = sqlx::query_scalar(
                    r#"
                    SELECT layout
                    FROM surfer_canvas_layout
                    WHERE private_beach_id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(tx.as_mut())
                .await?;

                let layout = if let Some(Json(value)) = row {
                    let layout: crate::routes::CanvasLayout = serde_json::from_value(value)?;
                    layout.ensure_version().map_err(StateError::InvalidLayout)?
                } else {
                    let migrated = legacy_layout_to_canvas(meta.settings.get("layout"), now_ms);
                    let payload = serde_json::to_value(&migrated)?;
                    sqlx::query(
                        r#"
                        INSERT INTO surfer_canvas_layout (private_beach_id, layout, updated_at)
                        VALUES ($1, $2, NOW())
                        ON CONFLICT (private_beach_id)
                        DO UPDATE SET layout = EXCLUDED.layout, updated_at = NOW()
                        "#,
                    )
                    .bind(id)
                    .bind(Json(payload))
                    .execute(tx.as_mut())
                    .await?;
                    migrated
                };

                tx.commit().await?;
                Ok(layout)
            }
        }
    }

    pub async fn put_private_beach_layout(
        &self,
        id_str: &str,
        layout: crate::routes::CanvasLayout,
        account: Option<Uuid>,
    ) -> Result<crate::routes::CanvasLayout, StateError> {
        let now_ms = Utc::now().timestamp_millis();
        let layout = layout
            .ensure_version()
            .map_err(StateError::InvalidLayout)?
            .with_updated_timestamp(now_ms);

        let pool = match &self.backend {
            Backend::Postgres(p) => p,
            Backend::Memory => {
                self.fallback
                    .set_canvas_layout(id_str.to_string(), layout.clone())
                    .await;
                return Ok(layout);
            }
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

        let payload = serde_json::to_value(&layout)?;
        sqlx::query(
            r#"
            INSERT INTO surfer_canvas_layout (private_beach_id, layout, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (private_beach_id)
            DO UPDATE SET layout = EXCLUDED.layout, updated_at = NOW()
            "#,
        )
        .bind(id)
        .bind(Json(payload))
        .execute(tx.as_mut())
        .await?;
        tx.commit().await?;
        Ok(layout)
    }

    async fn build_agent_trace_context(
        &self,
        pool: &PgPool,
        identifiers: &DbSessionIdentifiers,
        actions: &[ActionCommand],
    ) -> Option<(Vec<String>, String)> {
        if !tracing::enabled!(Level::TRACE) {
            return None;
        }
        let agents = match self
            .fetch_agent_controller_sessions(
                pool,
                &identifiers.private_beach_id,
                identifiers.session_id,
            )
            .await
        {
            Ok(list) => list,
            Err(err) => {
                debug!(
                    target = "agent_controller_comms",
                    session_id = %identifiers.session_id,
                    error = %err,
                    "failed to resolve agent controllers for trace logging"
                );
                return None;
            }
        };
        if agents.is_empty() {
            return None;
        }
        match serde_json::to_string(actions) {
            Ok(payload) => Some((agents, payload)),
            Err(err) => {
                debug!(
                    target = "agent_controller_comms",
                    session_id = %identifiers.session_id,
                    error = %err,
                    "failed to serialize action payload for trace logging"
                );
                None
            }
        }
    }

    async fn fetch_agent_controller_sessions(
        &self,
        pool: &PgPool,
        private_beach_id: &Uuid,
        child_session_id: Uuid,
    ) -> Result<Vec<String>, StateError> {
        let mut tx = pool.begin().await?;
        self.set_rls_context_tx(&mut tx, private_beach_id).await?;
        let rows = sqlx::query_as::<_, ControllerAgentSessionRow>(
            r#"
            SELECT controller.origin_session_id AS controller_origin_session_id,
                   controller.metadata AS controller_metadata
            FROM controller_pairing cp
            INNER JOIN session controller ON controller.id = cp.controller_session_id
            WHERE cp.child_session_id = $1
            "#,
        )
        .bind(child_session_id)
        .fetch_all(tx.as_mut())
        .await?;
        tx.commit().await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let metadata = row.controller_metadata.as_ref().map(|json| &json.0);
                if metadata_role_is_agent(metadata) {
                    Some(row.controller_origin_session_id.to_string())
                } else {
                    None
                }
            })
            .collect())
    }

    fn log_agent_bridge_payload(
        agent_sessions: &[String],
        child_session_id: &Uuid,
        private_beach_id: &Uuid,
        payload: &str,
        direction: &'static str,
        transport: &'static str,
    ) {
        if agent_sessions.is_empty() || !tracing::enabled!(Level::TRACE) {
            return;
        }
        let joined = agent_sessions.join(",");
        trace!(
            target = "agent_controller_comms",
            direction = direction,
            transport = transport,
            agent_sessions = %joined,
            child_session_id = %child_session_id,
            private_beach_id = %private_beach_id,
            bytes = payload.as_bytes().len(),
            payload = %payload,
            "agent harness communication"
        );
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
    join_code: String,
    road_base_url: String,
) {
    #[cfg(test)]
    if let Some(override_fn) = test_support::viewer_worker_override() {
        override_fn(
            state,
            session_id,
            private_beach_id,
            join_code,
            road_base_url,
        )
        .await;
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
            &join_code,
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
    join_code: &str,
    road_base_url: &str,
    label: &str,
) -> Result<(), ViewerError> {
    let gauge =
        metrics::MANAGER_VIEWER_CONNECTED.with_label_values(&[private_beach_id, session_id]);
    gauge.set(0);
    let gauge_guard = ViewerGaugeGuard::new(gauge.clone());
    let latency_hist =
        metrics::MANAGER_VIEWER_LATENCY_MS.with_label_values(&[private_beach_id, session_id]);

    let viewer_token = match state
        .viewer_token(session_id, private_beach_id, join_code)
        .await
    {
        Ok(issued) => Some(issued.token),
        Err(ViewerTokenError::Unavailable) => {
            debug!(
                target = "private_beach",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                "viewer token service unavailable; falling back to passcode only"
            );
            None
        }
        Err(err) => return Err(ViewerError::Credential(err)),
    };

    let config = SessionConfig::new(road_base_url).map_err(ViewerError::Join)?;
    let fallback_base = config.base_url().clone();
    let manager = SessionManager::new(config).map_err(ViewerError::Join)?;
    let joined = manager
        .join(
            session_id,
            Some(join_code),
            viewer_token.as_deref(),
            Some(label),
            false,
        )
        .await
        .map_err(ViewerError::Join)?;
    let mut handle = joined.into_handle();
    rewrite_loopback_transports(&mut handle, &fallback_base)?;
    let negotiated = negotiate_transport(&handle, Some(join_code), Some(label), false)
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

    let mut viewer_state = ManagerViewerState::new();
    let mut next_keepalive = Instant::now() + VIEWER_KEEPALIVE_INTERVAL;
    let mut last_frame_at = Instant::now();
    let mut idle_warned = false;

    loop {
        let now = Instant::now();
        if now >= next_keepalive {
            metrics::MANAGER_VIEWER_KEEPALIVE_SENT
                .with_label_values(&[private_beach_id, session_id])
                .inc();
            if let Err(err) = transport.send_text(VIEWER_KEEPALIVE_PAYLOAD) {
                metrics::MANAGER_VIEWER_KEEPALIVE_FAILURES
                    .with_label_values(&[private_beach_id, session_id])
                    .inc();
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
            Ok(message) => {
                if idle_warned {
                    metrics::MANAGER_VIEWER_IDLE_RECOVERIES
                        .with_label_values(&[private_beach_id, session_id])
                        .inc();
                }
                last_frame_at = Instant::now();
                idle_warned = false;
                match message.payload {
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
                                    .as_millis()
                                    as u64;
                                if now_ms >= *timestamp_ms {
                                    let latency_ms = now_ms - *timestamp_ms;
                                    latency_hist.observe(latency_ms as f64);
                                }
                            }
                            if let WireHostFrame::Grid {
                                history_rows,
                                base_row,
                                ..
                            } = &frame
                            {
                                if let Some(request) =
                                    viewer_state.take_history_request(*history_rows, *base_row)
                                {
                                    let payload = encode_client_frame_binary(&request);
                                    if let Err(err) = transport.send_bytes(&payload) {
                                        warn!(
                                            target = "private_beach",
                                            session_id = %session_id,
                                            error = %err,
                                            "manager viewer failed to request history backfill"
                                        );
                                    } else {
                                        debug!(
                                            target = "private_beach",
                                            session_id = %session_id,
                                            start = 0,
                                            rows = history_rows,
                                            "manager viewer requested terminal backfill"
                                        );
                                    }
                                }
                            }
                            if let Some(diff) = viewer_state.handle_host_frame(&frame) {
                                let sequence = diff.sequence;
                                if let Err(err) = state.record_state(session_id, diff, false).await
                                {
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
                                private_beach_id = %private_beach_id,
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
                }
            }
            Err(TransportError::Timeout) => {
                if !idle_warned {
                    let idle_duration = Instant::now().duration_since(last_frame_at);
                    if idle_duration >= VIEWER_IDLE_LOG_AFTER {
                        metrics::MANAGER_VIEWER_IDLE_WARNINGS
                            .with_label_values(&[private_beach_id, session_id])
                            .inc();
                        idle_warned = true;
                        warn!(
                            target = "private_beach",
                            session_id = %session_id,
                            private_beach_id = %private_beach_id,
                            idle_ms = idle_duration.as_millis(),
                            "manager viewer has been idle without frames"
                        );
                    }
                }
                continue;
            }
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

fn rewrite_loopback_transports(
    handle: &mut SessionHandle,
    fallback_base: &Url,
) -> Result<(), ViewerError> {
    if fallback_base.host_str().is_none() {
        return Err(viewer_config_error(
            "BEACH_ROAD_URL must include a host component",
        ));
    }

    if handle
        .session_url
        .host_str()
        .map_or(false, is_loopback_host)
    {
        let mut updated = handle.session_url.clone();
        rewrite_url_host_port(&mut updated, fallback_base)?;
        handle.session_url = updated;
    }

    for offer in &mut handle.offers {
        match offer {
            TransportOffer::WebSocket { url } | TransportOffer::WebSocketFallback { url } => {
                if let Ok(mut parsed) = Url::parse(url) {
                    if parsed.host_str().map_or(false, is_loopback_host) {
                        rewrite_url_host_port(&mut parsed, fallback_base)?;
                        *url = parsed.to_string();
                    }
                }
            }
            TransportOffer::WebRtc { offer } => {
                rewrite_webrtc_offer(offer, fallback_base)?;
            }
            TransportOffer::Ipc => {}
        }
    }

    Ok(())
}

fn rewrite_webrtc_offer(
    value: &mut serde_json::Value,
    fallback_base: &Url,
) -> Result<(), ViewerError> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };

    if let Some(signaling_value) = object.get_mut("signaling_url") {
        if let Some(rewritten) = rewrite_string_url(signaling_value, fallback_base)? {
            *signaling_value = serde_json::Value::String(rewritten);
        }
    }

    if let Some(url_value) = object.get_mut("url") {
        if let Some(rewritten) = rewrite_string_url(url_value, fallback_base)? {
            *url_value = serde_json::Value::String(rewritten);
        }
    }

    Ok(())
}

fn rewrite_string_url(
    value: &serde_json::Value,
    fallback_base: &Url,
) -> Result<Option<String>, ViewerError> {
    let Some(raw) = value.as_str() else {
        return Ok(None);
    };

    let Ok(mut parsed) = Url::parse(raw) else {
        return Ok(None);
    };

    if !parsed.host_str().map_or(false, is_loopback_host) {
        return Ok(None);
    }

    rewrite_url_host_port(&mut parsed, fallback_base)?;
    Ok(Some(parsed.to_string()))
}

fn rewrite_url_host_port(url: &mut Url, fallback_base: &Url) -> Result<(), ViewerError> {
    let host = fallback_base
        .host_str()
        .ok_or_else(|| viewer_config_error("BEACH_ROAD_URL must include a host component"))?;

    url.set_host(Some(host))
        .map_err(|_| viewer_config_error("failed to rewrite transport host"))?;

    match fallback_base.port() {
        Some(port) => url
            .set_port(Some(port))
            .map_err(|_| viewer_config_error("failed to rewrite transport port"))?,
        None => url
            .set_port(None)
            .map_err(|_| viewer_config_error("failed to clear transport port"))?,
    }

    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn viewer_config_error(message: impl Into<String>) -> ViewerError {
    ViewerError::Join(SessionError::InvalidConfig(message.into()))
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
    #[error("viewer credential failure: {0}")]
    Credential(ViewerTokenError),
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
    use beach_client_core::cache::terminal::packed::{pack_color_default, pack_color_rgb, StyleId};
    use beach_client_core::protocol::{
        HostFrame as WireHostFrame, Lane, LaneBudgetFrame, SyncConfigFrame, Update as WireUpdate,
    };
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::SystemTime;
    use tokio::time::{sleep, timeout, Duration};

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
                    state.record_state(&session_id, diff, false).await.unwrap();
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

    #[test]
    fn manager_viewer_style_updates_survive_pipeline() {
        let mut viewer = ManagerViewerState::new();

        let config = SyncConfigFrame {
            snapshot_budgets: vec![LaneBudgetFrame {
                lane: Lane::Foreground,
                max_updates: 1024,
            }],
            delta_budget: 1024,
            heartbeat_ms: 1_000,
            initial_snapshot_lines: 0,
        };

        viewer.handle_host_frame(&WireHostFrame::Hello {
            subscription: 1,
            max_seq: 1,
            config,
            features: 0,
        });

        viewer.handle_host_frame(&WireHostFrame::Grid {
            cols: 80,
            history_rows: 0,
            base_row: 0,
            viewport_rows: Some(24),
        });

        let updates = vec![
            WireUpdate::Style {
                id: 0,
                seq: 1,
                fg: pack_color_rgb(255, 0, 0),
                bg: pack_color_default(),
                attrs: 0,
            },
            WireUpdate::Cell {
                row: 0,
                col: 0,
                seq: 2,
                cell: ((b'A' as u64) << 32) | 0,
            },
        ];

        let diff = viewer
            .handle_host_frame(&WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 2,
                has_more: false,
                updates,
                cursor: None,
            })
            .expect("diff emitted");

        let lines = diff
            .payload
            .get("lines")
            .and_then(|value| value.as_array())
            .expect("lines array present on diff payload");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.as_str())
                .any(|line| line.contains('A')),
            "terminal diff should include styled cell contents"
        );

        let styles = diff
            .payload
            .get("styles")
            .and_then(|value| value.as_array())
            .expect("styles array present on diff payload");
        assert!(
            styles.iter().any(|entry| {
                let id = entry.get("id").and_then(|v| v.as_u64());
                let fg = entry.get("fg").and_then(|v| v.as_u64());
                id == Some(0) && fg == Some(pack_color_rgb(255, 0, 0) as u64)
            }),
            "styles array should include foreground definition for id 0"
        );

        let styled_lines = diff
            .payload
            .get("styled_lines")
            .and_then(|value| value.as_array())
            .expect("styled_lines array present on diff payload");
        let first_row = styled_lines
            .first()
            .and_then(|row| row.as_array())
            .expect("first styled row array");
        let first_cell = first_row
            .first()
            .and_then(|cell| cell.as_object())
            .expect("styled cell shape");
        assert_eq!(
            first_cell.get("ch").and_then(|v| v.as_str()),
            Some("A"),
            "styled cell should capture character"
        );
        let style_payload = first_cell
            .get("style")
            .and_then(|v| v.as_object())
            .expect("styled cell should include style payload");
        assert_eq!(
            style_payload.get("id").and_then(|v| v.as_u64()),
            Some(0),
            "styled cell should include style id 0"
        );
        assert_eq!(
            style_payload.get("fg").and_then(|v| v.as_u64()),
            Some(pack_color_rgb(255, 0, 0) as u64),
            "styled cell should embed resolved foreground color"
        );
        assert_eq!(
            diff.payload.get("cols").and_then(|v| v.as_u64()),
            Some(80),
            "payload should report column count"
        );

        let style = viewer
            .grid
            .style_table
            .get(StyleId(0))
            .expect("style table entry for id 0");
        assert_eq!(style.fg, pack_color_rgb(255, 0, 0));
        assert_eq!(style.bg, pack_color_default());
    }

    struct OverrideGuard;

    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            test_support::clear_viewer_worker_override();
        }
    }
}

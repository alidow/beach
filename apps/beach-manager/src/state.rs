//! Control-plane state for the Beach Manager service.
//!
//! The **manager** refers to this Rust control plane (`apps/beach-manager`). A **controller**
//! is any Beach session whose Beach Buggy harness currently holds a controller lease via the
//! manager APIs. Controllers drive other sessions; non-controller harnesses simply stream
//! state into the manager until a lease is granted.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fmt,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::auth::{AuthConfig, AuthContext};
use crate::publish_token::{PublishTokenManager, SignedPublishToken};
use crate::routes::ShowcasePreflightResponse;
use crate::{
    log_throttle::{should_log_custom_event, should_log_queue_event, QueueLogKind},
    metrics,
};
use base64::{engine::general_purpose, Engine as _};
use beach_buggy::{
    AckStatus, ActionAck, ActionCommand, CellStylePayload, CursorPosition, HarnessType,
    HealthHeartbeat, RegisterSessionRequest, RegisterSessionResponse, StateDiff, StyleDefinition,
    StyledCell, TerminalFrame, TransportMode,
};
use beach_client_core::auth::config::AuthConfig as GateAuthConfig;
use beach_client_core::auth::gate::{BeachGateClient, TurnIceServer};
use beach_client_core::cache::terminal::packed::unpack_cell;
use beach_client_core::protocol::{ClientFrame, CursorFrame, Update as WireUpdate};
use beach_client_core::transport::webrtc::{transport_diagnostics, warm_session_key};
use beach_client_core::transport::{extensions, framed};
use beach_client_core::{
    decode_host_frame_binary, encode_client_frame_binary, negotiate_transport, CliError,
    HostFrame as WireHostFrame, NegotiatedSingle, NegotiatedTransport, PackedCell, Payload,
    SessionConfig, SessionError, SessionHandle, SessionManager, Style, StyleId, TerminalGrid,
    Transport, TransportError, TransportKind, TransportOffer, WebRtcChannels,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use prometheus::IntGauge;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::Sha1;
use sqlx::{types::Json, FromRow, PgPool, Row};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn, Level};
use url::Url;
use uuid::Uuid;

const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
const REDIS_ACTION_STREAM_MAXLEN: usize = 2_048;
const REDIS_TTL_SECONDS: usize = 120;
const REDIS_PENDING_RECLAIM_MAX: usize = 64;
const REDIS_PENDING_RECLAIM_MIN_IDLE_MS: i64 = 5_000;
const REDIS_ACTION_GROUP: &str = "controllers";
const REDIS_ACTION_CONSUMER_PREFIX: &str = "poller";
const VIEWER_KEEPALIVE_INTERVAL: StdDuration = StdDuration::from_secs(20);
const VIEWER_KEEPALIVE_PAYLOAD: &str = "__keepalive__";
const VIEWER_IDLE_LOG_AFTER: StdDuration = StdDuration::from_secs(45);
const STATE_STREAM_HEARTBEAT_INTERVAL: StdDuration = StdDuration::from_secs(2);
pub(crate) const STALE_SESSION_MAX_IDLE: StdDuration = StdDuration::from_secs(60);
#[allow(dead_code)]
pub(crate) const STALE_SESSION_SWEEP_INTERVAL: StdDuration = StdDuration::from_secs(5);
const CONTROLLER_CHANNEL_LABEL: &str = "mgr-actions";
const LEGACY_CONTROLLER_CHANNEL_LABEL: &str = "pb-controller";
const VIEWER_HEALTH_REPORT_INTERVAL: StdDuration = StdDuration::from_secs(15);
const MAX_PENDING_ACTIONS_PER_SESSION: usize = 500;
const STREAM_EVENT_NO_SUBSCRIBERS_LOG_KIND: &str = "stream_event_no_subscribers";
const STREAM_EVENT_NO_SUBSCRIBERS_LOG_SECS: u64 = 30;
const CONTROLLER_STRICT_GATING_ENV: &str = "CONTROLLER_STRICT_GATING";
const CONTROLLER_STRICT_GATING_DEFAULT_ENABLED: bool = true;
const CONTROLLER_HANDSHAKE_STALE_AFTER: StdDuration = StdDuration::from_secs(20);
const IDLE_PUBLISH_TOKEN_HINT_KEY: &str = "idlePublishToken";
const SHOWCASE_PREFLIGHT_CACHE_TTL: StdDuration = StdDuration::from_secs(30);

pub fn viewer_health_report_interval() -> StdDuration {
    VIEWER_HEALTH_REPORT_INTERVAL
}

fn env_flag_enabled(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn manager_bearer_token() -> Option<String> {
    for key in [
        "MANAGER_JWT",
        "BEACH_MANAGER_JWT",
        "BEACH_MANAGER_BEARER",
        "CLERK_MANAGER_JWT",
        "BEACH_GATE_VIEWER_TOKEN",
    ] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn build_gate_client(gate_url: Option<String>) -> Option<BeachGateClient> {
    let mut config = match GateAuthConfig::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            warn!(
                error = %err,
                "unable to load Beach Gate auth config; TURN credentials will be unavailable"
            );
            return None;
        }
    };
    if let Some(url) = gate_url.or_else(|| std::env::var("BEACH_GATE_URL").ok()) {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            match Url::parse(trimmed) {
                Ok(parsed) => config.gateway = parsed,
                Err(err) => {
                    warn!(
                        error = %err,
                        gate_url = trimmed,
                        "invalid Beach Gate url; TURN credentials will be unavailable"
                    );
                    return None;
                }
            }
        }
    }
    match BeachGateClient::new(config) {
        Ok(client) => Some(client),
        Err(err) => {
            warn!(error = %err, "failed to initialize Beach Gate client");
            None
        }
    }
}

#[derive(Clone)]
struct StateKeepaliveManager {
    tasks: Arc<RwLock<HashMap<String, StateHeartbeatHandle>>>,
}

struct StateHeartbeatHandle {
    cancel: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
}

#[derive(Clone)]
pub struct AppState {
    backend: Backend,
    fallback: Arc<InnerState>,
    redis: Option<Arc<redis::Client>>,
    auth: Arc<AuthContext>,
    publish_tokens: Arc<PublishTokenManager>,
    events: Arc<RwLock<HashMap<String, broadcast::Sender<StreamEvent>>>>,
    devtools_events: Arc<RwLock<HashMap<String, broadcast::Sender<DevtoolsTimelineEvent>>>>,
    viewer_workers: Arc<RwLock<HashMap<String, ViewerWorker>>>,
    controller_workers: Arc<RwLock<HashMap<String, ControllerForwarderWorker>>>,
    viewer_tokens: Option<ViewerTokenClient>,
    gate_client: Option<BeachGateClient>,
    http: reqwest::Client,
    road_base_url: String,
    public_manager_url: String,
    controller_strict_gating: bool,
    idle_snapshot_interval_ms: Option<u64>,
    controller_handshakes: Arc<RwLock<HashMap<String, ControllerHandshakeInfo>>>,
    state_keepalive: StateKeepaliveManager,
    showcase_preflight_cache: Arc<RwLock<HashMap<String, ShowcasePreflightCacheEntry>>>,
}

struct ControllerHandshakeInfo {
    control_id: String,
    dispatched_at: Instant,
}

struct ShowcasePreflightCacheEntry {
    response: ShowcasePreflightResponse,
    expires_at: Instant,
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
    pending_transport: RwLock<HashMap<String, PairingTransportStatus>>,
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
    controller_leases: HashMap<String, ControllerLeaseMemory>,
    viewer_passcode: Option<String>,
    lease_ttl_ms: u64,
    transport_hints: HashMap<String, serde_json::Value>,
    state_cache_url: Option<String>,
    pending_actions: VecDeque<ActionCommand>,
    controller_events: Vec<ControllerEvent>,
    last_health: Option<HealthHeartbeat>,
    last_health_at: Option<Instant>,
    last_state: Option<StateDiff>,
    attached_at_ms: Option<i64>,
    http_ready_since_ms: Option<i64>,
    transport_mode: TransportMode,
}

#[derive(Debug, Clone, Copy)]
struct SessionReadinessSnapshot {
    attached_at_ms: Option<i64>,
    http_ready_since_ms: Option<i64>,
    last_health_at: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerAutoAttachHint {
    private_beach_id: String,
    attach_code: String,
    manager_url: String,
    issued_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handshake_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdlePublishTokenHint {
    pub token: String,
    pub expires_at_ms: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

impl IdlePublishTokenHint {
    fn from_signed(signed: &SignedPublishToken) -> Self {
        Self {
            token: signed.token.clone(),
            expires_at_ms: signed.expires_at * 1000,
            scopes: signed.scopes.clone(),
        }
    }

    fn as_value(&self) -> serde_json::Value {
        serde_json::json!({
            "token": self.token,
            "expires_at_ms": self.expires_at_ms,
            "scopes": self.scopes,
        })
    }
}

#[derive(Debug, Clone)]
struct ControllerLeaseMemory {
    expires_at_ms: i64,
    controller_account_id: Option<String>,
    issued_by_account_id: Option<String>,
    reason: Option<String>,
}

struct ViewerWorker {
    handle: JoinHandle<()>,
    cancel: CancellationToken,
    peer_id: String,
}

struct ControllerForwarderWorker {
    handle: JoinHandle<()>,
    cancel: CancellationToken,
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
    #[serde(default, rename = "expires_in")]
    _expires_in: Option<u64>,
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
    last_health_report: Option<Instant>,
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
            last_health_report: None,
        }
    }

    fn claim_health_report_slot(&mut self, now: Instant) -> bool {
        match self.last_health_report {
            Some(previous) if now.duration_since(previous) < VIEWER_HEALTH_REPORT_INTERVAL => false,
            _ => {
                self.last_health_report = Some(now);
                true
            }
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
            | WireHostFrame::Extension { .. }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerCommandDropReason {
    MissingLease,
    SessionNotBound,
    ChildNotAttached,
    ChildOffline,
}

impl ControllerCommandDropReason {
    pub fn code(&self) -> &'static str {
        match self {
            ControllerCommandDropReason::MissingLease => "missing_lease",
            ControllerCommandDropReason::SessionNotBound => "session_not_bound",
            ControllerCommandDropReason::ChildNotAttached => "child_not_attached",
            ControllerCommandDropReason::ChildOffline => "child_offline",
        }
    }

    pub fn default_message(&self) -> &'static str {
        match self {
            ControllerCommandDropReason::ChildNotAttached => {
                "child session has not attached to the private beach"
            }
            ControllerCommandDropReason::ChildOffline => "child session is offline",
            ControllerCommandDropReason::SessionNotBound => "child session has no runtime binding",
            ControllerCommandDropReason::MissingLease => "controller lease missing or expired",
        }
    }
}

impl fmt::Display for ControllerCommandDropReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
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
    #[error("controller account missing or inactive")]
    AccountMissing(Uuid),
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
    #[allow(dead_code)]
    #[error("external service error: {0}")]
    External(String),
    #[allow(dead_code)]
    #[error("internal error: {0}")]
    Internal(String),
    #[error("pending controller action queue full for session {session_id} ({depth}/{limit})")]
    ActionQueueFull {
        session_id: String,
        private_beach_id: String,
        depth: usize,
        limit: usize,
    },
    #[error("controller command rejected ({reason})")]
    ControllerCommandRejected { reason: ControllerCommandDropReason },
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
pub enum PairingTransportKind {
    #[serde(rename = "webrtc")]
    Rtc,
    #[serde(rename = "http_fallback")]
    HttpFallback,
    #[serde(rename = "pending")]
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

    pub fn webrtc(now_ms: i64) -> Self {
        Self {
            transport: PairingTransportKind::Rtc,
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

    pub fn semantically_equals(&self, other: &Self) -> bool {
        self.transport == other.transport
            && self.latency_ms == other.latency_ms
            && self.last_error == other.last_error
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachHandshakeDisposition {
    Dispatch,
    Skip,
}

impl AttachHandshakeDisposition {
    fn dispatching(self) -> bool {
        matches!(self, AttachHandshakeDisposition::Dispatch)
    }
}

#[derive(Debug, Clone)]
pub struct AttachByCodeOutcome {
    pub session: SessionSummary,
    pub handshake_dispatched: bool,
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
    Devtools(DevtoolsTimelineEvent),
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
            StreamEvent::Devtools(event) => ("devtools_event", serde_json::to_string(event).ok()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DevtoolsTimelineEvent {
    pub id: String,
    pub timestamp_ms: i64,
    pub timeline: DevtoolsTimelineKind,
    pub step: String,
    pub level: DevtoolsEventLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl DevtoolsTimelineEvent {
    fn host(
        session_id: &str,
        step: &str,
        level: DevtoolsEventLevel,
        detail: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: now_ms(),
            timeline: DevtoolsTimelineKind::Host,
            step: step.to_string(),
            level,
            session_id: Some(session_id.to_string()),
            controller_session_id: None,
            child_session_id: None,
            detail,
        }
    }

    fn connector(
        controller_session_id: &str,
        child_session_id: &str,
        step: &str,
        level: DevtoolsEventLevel,
        detail: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp_ms: now_ms(),
            timeline: DevtoolsTimelineKind::Connector,
            step: step.to_string(),
            level,
            session_id: Some(child_session_id.to_string()),
            controller_session_id: Some(controller_session_id.to_string()),
            child_session_id: Some(child_session_id.to_string()),
            detail,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevtoolsTimelineKind {
    Viewer,
    Host,
    Connector,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevtoolsEventLevel {
    Info,
    Warn,
    Error,
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
    #[sqlx(rename = "controller_session_id")]
    _controller_session_id: Uuid,
    #[sqlx(rename = "child_session_id")]
    _child_session_id: Uuid,
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
            publish_tokens: Arc::new(PublishTokenManager::from_env()),
            events: Arc::new(RwLock::new(HashMap::new())),
            devtools_events: Arc::new(RwLock::new(HashMap::new())),
            viewer_workers: Arc::new(RwLock::new(HashMap::new())),
            controller_workers: Arc::new(RwLock::new(HashMap::new())),
            viewer_tokens: None,
            gate_client: None,
            http: reqwest::Client::new(),
            road_base_url: std::env::var("BEACH_ROAD_URL")
                .unwrap_or_else(|_| "https://api.beach.sh".into()),
            public_manager_url: std::env::var("PUBLIC_MANAGER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
            controller_strict_gating: env_flag_enabled(
                CONTROLLER_STRICT_GATING_ENV,
                CONTROLLER_STRICT_GATING_DEFAULT_ENABLED,
            ),
            idle_snapshot_interval_ms: None,
            controller_handshakes: Arc::new(RwLock::new(HashMap::new())),
            state_keepalive: StateKeepaliveManager::new(),
            showcase_preflight_cache: Arc::new(RwLock::new(HashMap::new())),
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
            publish_tokens: Arc::new(PublishTokenManager::from_env()),
            events: Arc::new(RwLock::new(HashMap::new())),
            devtools_events: Arc::new(RwLock::new(HashMap::new())),
            viewer_workers: Arc::new(RwLock::new(HashMap::new())),
            controller_workers: Arc::new(RwLock::new(HashMap::new())),
            viewer_tokens: None,
            gate_client: None,
            http: reqwest::Client::new(),
            road_base_url: std::env::var("BEACH_ROAD_URL")
                .unwrap_or_else(|_| "https://api.beach.sh".into()),
            public_manager_url: std::env::var("PUBLIC_MANAGER_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
            controller_strict_gating: env_flag_enabled(
                CONTROLLER_STRICT_GATING_ENV,
                CONTROLLER_STRICT_GATING_DEFAULT_ENABLED,
            ),
            idle_snapshot_interval_ms: None,
            controller_handshakes: Arc::new(RwLock::new(HashMap::new())),
            state_keepalive: StateKeepaliveManager::new(),
            showcase_preflight_cache: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn with_gate_client(mut self, gate_url: Option<String>) -> Self {
        self.gate_client = build_gate_client(gate_url);
        self
    }

    pub fn public_manager_url(&self) -> &str {
        &self.public_manager_url
    }

    pub fn controller_strict_gating(&self) -> bool {
        self.controller_strict_gating
    }

    pub fn with_controller_strict_gating(mut self, enabled: bool) -> Self {
        self.controller_strict_gating = enabled;
        self
    }

    pub fn with_idle_snapshot_interval(mut self, interval_ms: Option<u64>) -> Self {
        self.idle_snapshot_interval_ms = interval_ms;
        self
    }

    async fn ensure_controller_account_active(
        &self,
        account_id: &Uuid,
        context: &'static str,
        session_id: Option<&str>,
    ) -> Result<(), StateError> {
        if let Backend::Memory = self.backend {
            return Ok(());
        }
        let account = *account_id;
        let active = self.account_active(account).await?;
        if !active {
            warn!(
                target: "controller.leases",
                context,
                account_id = %account_id,
                session_id = session_id.unwrap_or("unknown"),
                "controller account missing or inactive"
            );
            return Err(StateError::AccountMissing(account));
        }
        Ok(())
    }

    pub async fn cached_showcase_preflight(
        &self,
        private_beach_id: &str,
        refresh: bool,
    ) -> Option<ShowcasePreflightResponse> {
        if refresh {
            return None;
        }
        let mut cache = self.showcase_preflight_cache.write().await;
        if let Some(entry) = cache.get(private_beach_id) {
            if entry.expires_at > Instant::now() {
                return Some(entry.response.clone());
            }
            cache.remove(private_beach_id);
        }
        None
    }

    pub async fn store_showcase_preflight(
        &self,
        private_beach_id: &str,
        response: ShowcasePreflightResponse,
    ) {
        let mut cache = self.showcase_preflight_cache.write().await;
        cache.insert(
            private_beach_id.to_string(),
            ShowcasePreflightCacheEntry {
                response,
                expires_at: Instant::now() + SHOWCASE_PREFLIGHT_CACHE_TTL,
            },
        );
    }

    async fn mark_session_http_ready(&self, session_id: &str) {
        self.fallback.mark_http_ready(session_id).await;
    }

    pub async fn session_transport_mode(&self, session_id: &str) -> TransportMode {
        self.fallback
            .transport_mode(session_id)
            .await
            .unwrap_or(TransportMode::FastPath)
    }

    pub async fn is_rtc_ready(&self, session_id: &str) -> bool {
        self.rtc_ready(session_id).await
    }

    async fn rtc_ready(&self, session_id: &str) -> bool {
        #[cfg(test)]
        if let Some(value) = test_support::rtc_ready_override(session_id) {
            return value;
        }
        let controllers = self.fallback.controllers_for_child(session_id).await;
        let ready = if controllers.is_empty() {
            self.fallback
                .peek_pending_transport_status(session_id)
                .await
                .map(|status| matches!(status.transport, PairingTransportKind::Rtc))
                .unwrap_or(false)
        } else {
            let mut any_ready = false;
            for controller in controllers {
                let pairings = self.fallback.list_pairings(&controller).await;
                if pairings.iter().any(|pairing| {
                    pairing.child_session_id == session_id
                        && matches!(
                            pairing.transport_status.as_ref().map(|s| s.transport),
                            Some(PairingTransportKind::Rtc)
                        )
                }) {
                    any_ready = true;
                    break;
                }
            }
            any_ready
        };
        if should_log_custom_event("rtc_ready_check", session_id, StdDuration::from_secs(5)) {
            trace!(
                target = "controller.transport_state",
                session_id = %session_id,
                ready,
                "WebRTC transport readiness evaluated"
            );
        }
        ready
    }

    pub async fn controller_handshake_pending(&self, session_id: &str) -> bool {
        self.handshake_matches(session_id, None).await
    }

    async fn handshake_matches(&self, session_id: &str, expected_id: Option<&str>) -> bool {
        let mut map = self.controller_handshakes.write().await;
        if let Some(info) = map.get(session_id) {
            if info.dispatched_at.elapsed() >= CONTROLLER_HANDSHAKE_STALE_AFTER {
                trace!(
                    target = "controller.fast_path_state",
                    session_id = %session_id,
                    elapsed_ms = info.dispatched_at.elapsed().as_millis() as u64,
                    "controller handshake tracker expired"
                );
                map.remove(session_id);
                return false;
            }
            if let Some(handshake_id) = expected_id {
                return info.control_id == handshake_id;
            }
            return true;
        }
        false
    }

    pub async fn controller_handshake_matches(&self, session_id: &str, handshake_id: &str) -> bool {
        self.handshake_matches(session_id, Some(handshake_id)).await
    }

    async fn mark_controller_handshake_dispatched(&self, session_id: &str, control_id: String) {
        let mut map = self.controller_handshakes.write().await;
        map.insert(
            session_id.to_string(),
            ControllerHandshakeInfo {
                control_id,
                dispatched_at: Instant::now(),
            },
        );
        trace!(
            target = "controller.fast_path_state",
            session_id = %session_id,
            "controller handshake marked in-progress"
        );
    }

    async fn clear_controller_handshake(&self, session_id: &str) {
        let mut map = self.controller_handshakes.write().await;
        if map.remove(session_id).is_some() {
            trace!(
                target = "controller.fast_path_state",
                session_id = %session_id,
                "controller handshake tracker cleared"
            );
        }
    }

    #[cfg(test)]
    pub async fn active_handshake_id_for_test(&self, session_id: &str) -> Option<String> {
        let map = self.controller_handshakes.read().await;
        map.get(session_id).map(|info| info.control_id.clone())
    }

    async fn enforce_controller_gate(
        &self,
        private_beach_id: &str,
        session_id: &str,
        controller_token: &str,
        lease_id: Option<Uuid>,
        actor_account_id: Option<Uuid>,
        snapshot_override: Option<SessionReadinessSnapshot>,
    ) -> Result<(), StateError> {
        if !self.controller_strict_gating() {
            return Ok(());
        }
        let readiness = match snapshot_override {
            Some(snapshot) => Some(snapshot),
            None => self.fallback.session_readiness_snapshot(session_id).await,
        };
        match self.evaluate_controller_gate(session_id, readiness).await {
            Ok(()) => Ok(()),
            Err(reason) => Err(self
                .controller_command_rejection_with_snapshot(
                    private_beach_id,
                    session_id,
                    controller_token,
                    lease_id,
                    actor_account_id,
                    reason,
                    readiness,
                )
                .await),
        }
    }

    async fn evaluate_controller_gate(
        &self,
        _session_id: &str,
        readiness: Option<SessionReadinessSnapshot>,
    ) -> Result<(), ControllerCommandDropReason> {
        let Some(snapshot) = readiness else {
            return Err(ControllerCommandDropReason::SessionNotBound);
        };
        if !snapshot.attached() {
            return Err(ControllerCommandDropReason::ChildNotAttached);
        }
        if snapshot.ever_reported_health() && !snapshot.child_online() {
            return Err(ControllerCommandDropReason::ChildOffline);
        }
        Ok(())
    }

    async fn controller_command_rejection_with_snapshot(
        &self,
        private_beach_id: &str,
        session_id: &str,
        controller_token: &str,
        lease_id: Option<Uuid>,
        actor_account_id: Option<Uuid>,
        reason: ControllerCommandDropReason,
        snapshot: Option<SessionReadinessSnapshot>,
    ) -> StateError {
        let readiness = match snapshot {
            Some(value) => Some(value),
            None => self.fallback.session_readiness_snapshot(session_id).await,
        };
        self.record_controller_command_drop(
            private_beach_id,
            session_id,
            controller_token,
            lease_id,
            actor_account_id,
            readiness.as_ref(),
            reason,
        )
        .await;
        StateError::ControllerCommandRejected { reason }
    }

    async fn record_controller_command_drop(
        &self,
        private_beach_id: &str,
        session_id: &str,
        controller_token: &str,
        lease_id: Option<Uuid>,
        actor_account_id: Option<Uuid>,
        readiness: Option<&SessionReadinessSnapshot>,
        reason: ControllerCommandDropReason,
    ) {
        metrics::CONTROLLER_ACTIONS_DROPPED
            .with_label_values(&[reason.code()])
            .inc();

        if let Some(snapshot) = readiness {
            if let Some(age) = snapshot.attach_age_seconds() {
                metrics::CONTROLLER_COMMAND_BLOCK_LATENCY
                    .with_label_values(&[reason.code()])
                    .observe(age);
            }
        }

        let lease_label = lease_id.map(|id| truncate_uuid(&id));
        let actor_label = actor_account_id.map(|id| id.to_string());
        let rtc_ready = self.rtc_ready(session_id).await;
        let attached = readiness.map(|s| s.attached()).unwrap_or(false);
        let http_ready = readiness.map(|s| s.http_ready()).unwrap_or(false);
        let child_online = readiness.map(|s| s.child_online()).unwrap_or(true);
        let attach_age = readiness
            .and_then(|s| s.attach_age_seconds())
            .unwrap_or_default();

        warn!(
            target = "controller.actions.drop",
            private_beach_id = %private_beach_id,
            session_id = %session_id,
            controller_token = %redact_controller_token(controller_token),
            lease_id = lease_label.as_deref().unwrap_or("none"),
            actor_account_id = actor_label.as_deref().unwrap_or("none"),
            reason = reason.code(),
            attached,
            http_ready,
            rtc_ready,
            child_online,
            attach_age_secs = attach_age,
            strict_gating = self.controller_strict_gating(),
            "dropping controller command"
        );
    }

    pub(crate) fn build_controller_auto_attach_hint(
        &self,
        private_beach_id: &str,
        attach_code: &str,
        handshake_id: Option<String>,
    ) -> ControllerAutoAttachHint {
        ControllerAutoAttachHint {
            private_beach_id: private_beach_id.to_string(),
            attach_code: attach_code.to_string(),
            manager_url: self.public_manager_url.clone(),
            issued_at: Utc::now(),
            expires_at: None,
            handshake_id,
        }
    }

    fn ensure_controller_auto_attach_hint(
        &self,
        record: &mut SessionRecord,
        private_beach_id: &str,
    ) -> Result<(), serde_json::Error> {
        if let Some(code) = record
            .viewer_passcode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let hint = self.build_controller_auto_attach_hint(private_beach_id, code, None);
            record.upsert_controller_auto_attach_hint(&hint)?;
        }
        Ok(())
    }

    async fn refresh_idle_publish_token_hint(
        &self,
        session_id: &str,
    ) -> Result<IdlePublishTokenHint, StateError> {
        let signed = self.publish_tokens.sign_for_session(session_id);
        let hint = IdlePublishTokenHint::from_signed(&signed);

        {
            let mut sessions = self.fallback.sessions.write().await;
            if let Some(record) = sessions.get_mut(session_id) {
                record.upsert_idle_publish_token_hint(&hint)?;
            }
        }

        if let Backend::Postgres(pool) = &self.backend {
            let session_uuid = parse_uuid(session_id, "session_id")?;
            let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
            let mut tx = pool.begin().await?;
            self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                .await?;
            let existing: Option<(Json<serde_json::Value>,)> = sqlx::query_as(
                r#"
                SELECT transport_hints
                FROM session_runtime
                WHERE session_id = $1
                FOR UPDATE
                "#,
            )
            .bind(identifiers.session_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let mut hints_map: HashMap<String, serde_json::Value> = existing
                .map(|(Json(value),)| serde_json::from_value(value))
                .transpose()?
                .unwrap_or_else(|| {
                    default_transport_hints(session_id, self.idle_snapshot_interval_ms)
                });
            inject_idle_publish_hint(&mut hints_map, &hint);
            let value = serde_json::to_value(&hints_map)?;
            sqlx::query(
                r#"
                INSERT INTO session_runtime (session_id, transport_hints)
                VALUES ($1, $2)
                ON CONFLICT (session_id) DO UPDATE SET transport_hints = EXCLUDED.transport_hints
                "#,
            )
            .bind(identifiers.session_id)
            .bind(Json(value))
            .execute(tx.as_mut())
            .await?;
            tx.commit().await?;
            self.fallback
                .set_transport_hints(session_id, hints_map)
                .await;
        }

        Ok(hint)
    }

    async fn clear_idle_publish_token_hint(&self, session_id: &str) -> Result<(), StateError> {
        {
            let mut sessions = self.fallback.sessions.write().await;
            if let Some(record) = sessions.get_mut(session_id) {
                record.clear_idle_publish_token_hint();
            }
        }

        if let Backend::Postgres(pool) = &self.backend {
            let session_uuid = parse_uuid(session_id, "session_id")?;
            let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
            let mut tx = pool.begin().await?;
            self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                .await?;
            let existing: Option<(Json<serde_json::Value>,)> = sqlx::query_as(
                r#"
                SELECT transport_hints
                FROM session_runtime
                WHERE session_id = $1
                FOR UPDATE
                "#,
            )
            .bind(identifiers.session_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let mut hints_map: HashMap<String, serde_json::Value> = existing
                .map(|(Json(value),)| serde_json::from_value(value))
                .transpose()?
                .unwrap_or_else(|| {
                    default_transport_hints(session_id, self.idle_snapshot_interval_ms)
                });
            remove_idle_publish_hint(&mut hints_map);
            let value = serde_json::to_value(&hints_map)?;
            sqlx::query(
                r#"
                INSERT INTO session_runtime (session_id, transport_hints)
                VALUES ($1, $2)
                ON CONFLICT (session_id) DO UPDATE SET transport_hints = EXCLUDED.transport_hints
                "#,
            )
            .bind(identifiers.session_id)
            .bind(Json(value))
            .execute(tx.as_mut())
            .await?;
            tx.commit().await?;
            self.fallback
                .set_transport_hints(session_id, hints_map)
                .await;
        }

        Ok(())
    }

    pub(crate) async fn load_idle_publish_token_hint(
        &self,
        session_id: &str,
    ) -> Result<Option<IdlePublishTokenHint>, StateError> {
        {
            let sessions = self.fallback.sessions.read().await;
            if let Some(record) = sessions.get(session_id) {
                if let Some(value) = record
                    .transport_hints
                    .get(IDLE_PUBLISH_TOKEN_HINT_KEY)
                    .cloned()
                {
                    if let Ok(parsed) = serde_json::from_value(value) {
                        return Ok(Some(parsed));
                    }
                }
            }
        }
        match &self.backend {
            Backend::Memory => Ok(None),
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let row: Option<(Json<serde_json::Value>,)> = sqlx::query_as(
                    r#"
                    SELECT transport_hints
                    FROM session_runtime
                    WHERE session_id = $1
                    "#,
                )
                .bind(identifiers.session_id)
                .fetch_optional(pool)
                .await?;
                if let Some((Json(value),)) = row {
                    if let Ok(map) =
                        serde_json::from_value::<HashMap<String, serde_json::Value>>(value)
                    {
                        if let Some(token_value) = map.get(IDLE_PUBLISH_TOKEN_HINT_KEY) {
                            if let Ok(parsed) = serde_json::from_value(token_value.clone()) {
                                return Ok(Some(parsed));
                            }
                        }
                    }
                }
                Ok(None)
            }
        }
    }

    async fn prepare_transport_hints_for_registration(
        &self,
        req: &RegisterSessionRequest,
    ) -> Result<HashMap<String, serde_json::Value>, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let idle_interval = self.idle_snapshot_interval_ms;
        let transport_mode = req.transport_mode.unwrap_or(TransportMode::FastPath);
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(
                &req.session_id,
                &req.private_beach_id,
                &req.harness_type,
                idle_interval,
                transport_mode,
            )
        });
        entry.transport_mode = transport_mode;
        entry.viewer_passcode = req.viewer_passcode.clone();
        self.ensure_controller_auto_attach_hint(entry, &req.private_beach_id)?;
        // Attach a session-scoped publish token for idle snapshots and health.
        let hint = IdlePublishTokenHint::from_signed(
            &self.publish_tokens.sign_for_session(&req.session_id),
        );
        entry.upsert_idle_publish_token_hint(&hint)?;
        Ok(entry.transport_hints.clone())
    }

    async fn send_manager_handshake(
        &self,
        session_id: &str,
        private_beach_id: &str,
        passcode: &str,
    ) -> Result<(), StateError> {
        let handshake_control_id = Uuid::new_v4().to_string();
        let lease = self
            .acquire_controller(session_id, None, Some("auto_handshake".into()), None)
            .await?;

        let mut handshake = serde_json::json!({
            "private_beach_id": private_beach_id,
            "manager_url": self.public_manager_url(),
            "controller_token": lease.controller_token,
            "lease_expires_at_ms": lease.expires_at_ms,
            "stale_session_idle_secs": STALE_SESSION_MAX_IDLE.as_secs(),
            "viewer_health_interval_secs": viewer_health_report_interval().as_secs(),
            "controller_auto_attach": self.build_controller_auto_attach_hint(
                private_beach_id,
                passcode,
                Some(handshake_control_id.clone()),
            ),
            "handshake_control_id": handshake_control_id,
        });
        let publish_hint = self.refresh_idle_publish_token_hint(session_id).await?;
        if let Some(obj) = handshake.as_object_mut() {
            obj.insert(IDLE_PUBLISH_TOKEN_HINT_KEY.into(), publish_hint.as_value());
            if let Some(interval) = self.idle_snapshot_interval_ms {
                obj.insert(
                    "idle_snapshot".into(),
                    serde_json::json!({
                        "interval_ms": interval,
                        "mode": "terminal_full",
                        "publish_token": publish_hint.as_value()
                    }),
                );
            }
        }
        #[cfg(test)]
        test_support::record_manager_handshake(session_id, &handshake);

        // Throttled trace to help correlate manager-side handshake construction
        // with host/agent behavior without leaking credentials.
        let has_auto_attach = handshake
            .get("controller_auto_attach")
            .and_then(|value| value.as_object())
            .is_some();
        let idle_snapshot_interval_ms = handshake
            .get("idle_snapshot")
            .and_then(|value| value.get("interval_ms"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        info!(
            target = "controller.actions",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            has_auto_attach,
            idle_snapshot_interval_ms,
            "manager handshake prepared"
        );

        let url = format!(
            "{}/sessions/{}/control",
            self.road_base_url.trim_end_matches('/'),
            session_id
        );
        let body = serde_json::json!({
            "kind": "manager_handshake",
            "payload": handshake,
        });

        info!(
            target = "controller.actions",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            "manager handshake prepared for wss dispatch only (HTTP disabled)"
        );
        self.mark_controller_handshake_dispatched(session_id, handshake_control_id)
            .await;

        Ok(())
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

    pub fn publish_token_manager(&self) -> Arc<PublishTokenManager> {
        self.publish_tokens.clone()
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

    pub async fn subscribe_devtools(
        &self,
        session_id: &str,
    ) -> broadcast::Receiver<DevtoolsTimelineEvent> {
        let mut map = self.devtools_events.write().await;
        let tx = map
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        tx.subscribe()
    }

    async fn clear_session_stream(&self, session_id: &str) {
        let mut map = self.events.write().await;
        if map.remove(session_id).is_some() {
            debug!(session_id = %session_id, "session stream cleared");
        }
        {
            let mut devtools = self.devtools_events.write().await;
            if devtools.remove(session_id).is_some() {
                debug!(session_id = %session_id, "devtools stream cleared");
            }
        }
        self.state_keepalive.cancel(session_id).await;
    }

    async fn publish(&self, session_id: &str, event: StreamEvent) {
        let tx_opt = { self.events.read().await.get(session_id).cloned() };
        if let Some(tx) = tx_opt {
            let (event_kind, _) = event.as_named_json();
            if tx.receiver_count() == 0 {
                self.log_no_stream_subscribers(session_id, event_kind);
                return;
            }
            if tx.send(event).is_err() {
                self.log_no_stream_subscribers(session_id, event_kind);
            }
        }
    }

    async fn publish_devtools_event(&self, session_id: &str, event: DevtoolsTimelineEvent) {
        let tx_opt = { self.devtools_events.read().await.get(session_id).cloned() };
        if let Some(tx) = tx_opt {
            if tx.receiver_count() == 0 {
                return;
            }
            if tx.send(event).is_err() {
                // drop silently when no listeners
            }
        }
    }

    async fn emit_host_devtools_event(
        &self,
        session_id: &str,
        step: &str,
        level: DevtoolsEventLevel,
        detail: Option<serde_json::Value>,
    ) {
        let event = DevtoolsTimelineEvent::host(session_id, step, level, detail);
        self.publish_devtools_event(session_id, event).await;
    }

    async fn emit_connector_events_for_child(
        &self,
        child_session_id: &str,
        step: &str,
        level: DevtoolsEventLevel,
        detail: Option<serde_json::Value>,
    ) {
        let controllers = self.fallback.controllers_for_child(child_session_id).await;
        if controllers.is_empty() {
            debug!(
                target = "devtools.connector",
                child_session_id = %child_session_id,
                step,
                "no controller pairings registered; skipping connector event"
            );
            return;
        }
        for controller_session_id in controllers {
            let event = DevtoolsTimelineEvent::connector(
                &controller_session_id,
                child_session_id,
                step,
                level,
                detail.clone(),
            );
            self.publish_devtools_event(&controller_session_id, event.clone())
                .await;
            self.publish_devtools_event(child_session_id, event).await;
        }
    }

    fn log_no_stream_subscribers(&self, session_id: &str, event_kind: &'static str) {
        if should_log_custom_event(
            STREAM_EVENT_NO_SUBSCRIBERS_LOG_KIND,
            session_id,
            StdDuration::from_secs(STREAM_EVENT_NO_SUBSCRIBERS_LOG_SECS),
        ) {
            info!(
                session_id = %session_id,
                event_kind,
                "no subscribers to receive stream event"
            );
        }
    }

    async fn publish_state_heartbeat(&self, session_id: &str, sequence: u64) {
        if !self.has_stream_subscribers(session_id).await {
            return;
        }
        let heartbeat = StateDiff {
            sequence,
            emitted_at: SystemTime::now(),
            payload: serde_json::json!({
                "type": "terminal_heartbeat"
            }),
        };
        self.publish(session_id, StreamEvent::State(heartbeat))
            .await;
    }

    async fn has_stream_subscribers(&self, session_id: &str) -> bool {
        self.events
            .read()
            .await
            .get(session_id)
            .map(|tx| tx.receiver_count() > 0)
            .unwrap_or(false)
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

    pub async fn update_pairing_transport_status(
        &self,
        child_session_id: &str,
        status: PairingTransportStatus,
    ) {
        let previous = self
            .fallback
            .peek_pending_transport_status(child_session_id)
            .await;
        trace!(
            target = "controller.transport_status",
            child_session_id = %child_session_id,
            previous = ?previous.as_ref().map(|s| s.transport),
            new_transport = ?status.transport,
            new_latency = status.latency_ms,
            new_error = status.last_error,
            "update_pairing_transport_status"
        );
        let controllers = self.fallback.controllers_for_child(child_session_id).await;
        if controllers.is_empty() {
            if let Some(children) = self
                .fallback
                .children_for_controller(child_session_id)
                .await
            {
                for child in children {
                    let controllers = self.fallback.controllers_for_child(&child).await;
                    if controllers.is_empty() {
                        self.fallback
                            .record_pending_transport_status(&child, status.clone())
                            .await;
                        continue;
                    }
                    self.apply_transport_status_for_child(&child, controllers, status.clone())
                        .await;
                }
                return;
            }
            self.fallback
                .record_pending_transport_status(child_session_id, status.clone())
                .await;
            trace!(
                target = "controller.transport_status",
                child_session_id = %child_session_id,
                transport = ?status.transport,
                "no controller pairing; cached transport status for replay"
            );
            return;
        }
        self.apply_transport_status_for_child(child_session_id, controllers, status)
            .await;
    }

    async fn apply_transport_status_for_child(
        &self,
        child_session_id: &str,
        controllers: Vec<String>,
        status: PairingTransportStatus,
    ) {
        let status_clone = status.clone();

        // Drop any stale cached status now that a pairing is available.
        let _ = self
            .fallback
            .take_pending_transport_status(child_session_id)
            .await;

        for controller_session_id in controllers {
            if let Some(updated) = self
                .fallback
                .update_pairing_status(
                    &controller_session_id,
                    child_session_id,
                    status_clone.clone(),
                )
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
                self.emit_connector_events_for_child(
                    child_session_id,
                    "connector.transport:update",
                    DevtoolsEventLevel::Info,
                    Some(serde_json::json!({
                        "controllerSessionId": controller_session_id,
                        "transport": status_clone.transport,
                        "latencyMs": status_clone.latency_ms,
                        "error": status_clone.last_error,
                    })),
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
        handshake: AttachHandshakeDisposition,
    ) -> Result<AttachByCodeOutcome, StateError> {
        let skip_verify = std::env::var("BEACH_SKIP_ROAD_VERIFY")
            .ok()
            .map(|v| v.trim().eq_ignore_ascii_case("1") || v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let skip_workers = std::env::var("BEACH_TEST_DISABLE_SESSION_WORKERS")
            .ok()
            .map(|v| v.trim().eq_ignore_ascii_case("1") || v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !skip_verify {
            // Verify with Beach Road
            let verified = self
                .verify_code_with_road(origin_session_id, code)
                .await
                .unwrap_or(false);
            if !verified {
                warn!(
                    target = "private_beach.sessions",
                    private_beach_id = %private_beach_id,
                    session_id = %origin_session_id,
                    "attach_by_code verification failed"
                );
                return Err(StateError::InvalidIdentifier("invalid_code".into()));
            }
            info!(
                target = "private_beach.sessions",
                private_beach_id = %private_beach_id,
                session_id = %origin_session_id,
                "attach_by_code verification succeeded"
            );
        } else {
            info!(
                target = "private_beach.sessions",
                private_beach_id = %private_beach_id,
                session_id = %origin_session_id,
                "attach_by_code verification skipped via env override"
            );
        }
        // Create mapping if not exists
        match &self.backend {
            Backend::Memory => {
                let (summary, existed) = {
                    let mut sessions = self.fallback.sessions.write().await;
                    let existed = sessions.contains_key(origin_session_id);
                    let rec = sessions
                        .entry(origin_session_id.to_string())
                        .or_insert_with(|| {
                            SessionRecord::new(
                                origin_session_id,
                                private_beach_id,
                                &HarnessType::Custom,
                                self.idle_snapshot_interval_ms,
                                TransportMode::FastPath,
                            )
                        });
                    rec.viewer_passcode = Some(code.to_string());
                    rec.mark_attached();
                    let hint = self.build_controller_auto_attach_hint(private_beach_id, code, None);
                    rec.upsert_controller_auto_attach_hint(&hint)?;
                    let token = rec.first_lease_token();
                    rec.append_event(
                        ControllerEventType::Registered,
                        token,
                        Some("attach_by_code".into()),
                    );
                    (SessionSummary::from_record(rec), existed)
                };
                if skip_workers {
                    debug!(
                        target = "private_beach.sessions",
                        session_id = %origin_session_id,
                        "skipping session worker spawn via test override"
                    );
                } else {
                    if let Err(err) = self.spawn_viewer_worker(origin_session_id).await {
                        warn!(
                            target = "private_beach",
                            session_id = %origin_session_id,
                            error = %err,
                            "failed to start viewer worker after attach_by_code (memory backend)"
                        );
                    }
                    if let Err(err) = self.spawn_controller_forwarder(origin_session_id).await {
                        warn!(
                            target = "controller.forwarder",
                            session_id = %origin_session_id,
                            error = %err,
                            "failed to start controller forwarder (memory backend)"
                        );
                    }
                }
                let mut dispatched_handshake = false;
                if handshake.dispatching() {
                    match self
                        .send_manager_handshake(origin_session_id, private_beach_id, code)
                        .await
                    {
                        Ok(_) => {
                            dispatched_handshake = true;
                        }
                        Err(err) => {
                            warn!(
                                target = "controller.actions",
                                session_id = %origin_session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                "failed to dispatch manager handshake after attach_by_code"
                            );
                        }
                    }
                } else {
                    debug!(
                        target = "controller.actions",
                        session_id = %origin_session_id,
                        private_beach_id = %private_beach_id,
                        "skipping manager handshake dispatch for attach_by_code"
                    );
                }
                log_session_attachment(
                    private_beach_id,
                    origin_session_id,
                    "code",
                    if existed { "updated" } else { "attached" },
                );
                self.refresh_idle_publish_token_hint(origin_session_id)
                    .await?;
                Ok(AttachByCodeOutcome {
                    session: summary,
                    handshake_dispatched: dispatched_handshake,
                })
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
                let transport_hints_json = {
                    let mut sessions = self.fallback.sessions.write().await;
                    let rec = sessions
                        .entry(origin_session_id.to_string())
                        .or_insert_with(|| {
                            SessionRecord::new(
                                origin_session_id,
                                private_beach_id,
                                &HarnessType::Custom,
                                self.idle_snapshot_interval_ms,
                                TransportMode::FastPath,
                            )
                        });
                    rec.viewer_passcode = Some(code.to_string());
                    rec.upsert_controller_auto_attach_hint(
                        &self.build_controller_auto_attach_hint(private_beach_id, code, None),
                    )?;
                    serde_json::to_value(&rec.transport_hints)?
                };
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
                let insert_result = sqlx::query(
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
                let inserted = insert_result.rows_affected() > 0;

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
                    INSERT INTO session_runtime (session_id, viewer_passcode, transport_hints)
                    VALUES ($1, $2, $3)
                    ON CONFLICT (session_id)
                    DO UPDATE SET
                        viewer_passcode = EXCLUDED.viewer_passcode,
                        transport_hints = EXCLUDED.transport_hints
                    "#,
                )
                .bind(ids.session_id)
                .bind(code)
                .bind(Json(transport_hints_json.clone()))
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
                                self.idle_snapshot_interval_ms,
                                TransportMode::FastPath,
                            )
                        });
                    rec.viewer_passcode = Some(code.to_string());
                    rec.mark_attached();
                }
                log_session_attachment(
                    private_beach_id,
                    origin_session_id,
                    "code",
                    if inserted { "attached" } else { "updated" },
                );

                if skip_workers {
                    debug!(
                        target = "private_beach.sessions",
                        session_id = %origin_session_id,
                        "skipping session worker spawn via test override"
                    );
                } else {
                    if let Err(err) = self.spawn_viewer_worker(origin_session_id).await {
                        warn!(
                            target = "private_beach",
                            session_id = %origin_session_id,
                            error = %err,
                            "failed to start viewer worker after attach_by_code"
                        );
                    }
                    if let Err(err) = self.spawn_controller_forwarder(origin_session_id).await {
                        warn!(
                            target = "controller.forwarder",
                            session_id = %origin_session_id,
                            error = %err,
                            "failed to start controller forwarder"
                        );
                    }
                }
                let mut dispatched_handshake = false;
                if handshake.dispatching() {
                    match self
                        .send_manager_handshake(origin_session_id, private_beach_id, code)
                        .await
                    {
                        Ok(_) => {
                            dispatched_handshake = true;
                        }
                        Err(err) => {
                            warn!(
                                target = "controller.actions",
                                session_id = %origin_session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                "failed to dispatch manager handshake after attach_by_code"
                            );
                        }
                    }
                } else {
                    debug!(
                        target = "controller.actions",
                        session_id = %origin_session_id,
                        private_beach_id = %private_beach_id,
                        "skipping manager handshake dispatch for attach_by_code"
                    );
                }

                // Return summary (best-effort from DB fields)
                let list = self.list_sessions(private_beach_id).await?;
                if let Some(found) = list.iter().find(|s| s.session_id == origin_session_id) {
                    self.refresh_idle_publish_token_hint(origin_session_id)
                        .await?;
                    return Ok(AttachByCodeOutcome {
                        session: found.clone(),
                        handshake_dispatched: dispatched_handshake,
                    });
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
                        SessionRecord::new(
                            &id,
                            private_beach_id,
                            &HarnessType::Custom,
                            self.idle_snapshot_interval_ms,
                            TransportMode::FastPath,
                        )
                    });
                    entry.mark_attached();
                    if existed {
                        duplicates += 1;
                    } else {
                        attached += 1;
                        let token = entry.first_lease_token();
                        entry.append_event(
                            ControllerEventType::Registered,
                            token,
                            Some("attach_owned".into()),
                        );
                    }
                    log_session_attachment(
                        private_beach_id,
                        &id,
                        "owned",
                        if existed { "duplicate" } else { "attached" },
                    );
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
                            log_session_attachment(private_beach_id, &id, "owned", "duplicate");
                        } else {
                            attached += 1;
                            ids_to_nudge.push(id.clone());
                            log_session_attachment(private_beach_id, &id, "owned", "attached");
                        }
                        self.fallback.mark_attached(&id).await;
                    }
                }
                tx.commit().await?;
                Ok((attached, duplicates))
            }
        }
    }

    pub(crate) async fn verify_code_with_road(
        &self,
        origin_session_id: &str,
        code: &str,
    ) -> Result<bool, ()> {
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

    pub async fn gate_turn_credentials(&self) -> Option<(Vec<TurnIceServer>, i64)> {
        let client = self.gate_client.as_ref()?;
        let bearer = manager_bearer_token()?;
        let trimmed = bearer.trim();
        if trimmed.is_empty() {
            return None;
        }

        let creds = match client.turn_credentials(trimmed).await {
            Ok(value) => value,
            Err(err) => {
                warn!(error = %err, "Beach Gate TURN credentials fetch failed");
                return None;
            }
        };

        let expires_at_ms = creds
            .expires_at
            .map(|value| value.saturating_mul(1000))
            .unwrap_or_else(|| {
                let ttl_ms: i64 = creds.ttl_seconds.saturating_mul(1000) as i64;
                now_ms() + ttl_ms
            });
        let ice_servers: Vec<TurnIceServer> = creds
            .ice_servers
            .into_iter()
            .filter(|server| !server.urls.is_empty())
            .collect();
        if ice_servers.is_empty() {
            warn!("Beach Gate returned empty TURN servers; skipping attach");
            return None;
        }
        Some((ice_servers, expires_at_ms))
    }

    pub async fn gate_turn_credentials_or_dev_fallback(&self) -> Option<(Vec<TurnIceServer>, i64)> {
        if let Some(value) = self.gate_turn_credentials().await {
            return Some(value);
        }
        // Dev fallback: allow generating TURN creds locally when manager bearer is missing/invalid and insecure token is allowed.
        if std::env::var("DEV_ALLOW_INSECURE_MANAGER_TOKEN").unwrap_or_default() != "1" {
            return None;
        }
        let urls_env = std::env::var("BEACH_GATE_TURN_URLS").ok()?;
        let urls: Vec<String> = urls_env
            .split(',')
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
            .collect();
        if urls.is_empty() {
            return None;
        }
        let secret = std::env::var("BEACH_GATE_TURN_SECRET").ok()?;
        let ttl_seconds: i64 = std::env::var("BEACH_GATE_TURN_TTL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(14_400);
        let expires_at_ms = now_ms() + ttl_seconds.saturating_mul(1000);
        let username = format!("{}:dev", expires_at_ms / 1000);

        type HmacSha1 = Hmac<Sha1>;
        let mut mac = match HmacSha1::new_from_slice(secret.as_bytes()) {
            Ok(m) => m,
            Err(err) => {
                warn!(error = %err, "failed to init HMAC for dev TURN creds");
                return None;
            }
        };
        mac.update(username.as_bytes());
        let credential = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let ice_servers = vec![TurnIceServer {
            urls,
            username: Some(username),
            credential: Some(credential),
            credential_type: Some("password".into()),
        }];

        Some((ice_servers, expires_at_ms))
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
                        lease.id AS controller_token,
                        lease.expires_at,
                        lease.revoked_at
                    FROM session s
                    LEFT JOIN session_runtime sr ON sr.session_id = s.id
                    LEFT JOIN LATERAL (
                        SELECT cl.id, cl.expires_at, cl.revoked_at
                        FROM controller_lease cl
                        WHERE cl.session_id = s.id
                          AND cl.revoked_at IS NULL
                          AND cl.expires_at > NOW()
                        ORDER BY cl.expires_at DESC
                        LIMIT 1
                    ) AS lease ON TRUE
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
        if let Some(account_id) = actor_account_id.as_ref() {
            self.ensure_controller_account_active(
                account_id,
                "controller_pairing_upsert",
                Some(controller_session_id),
            )
            .await?;
        }
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

                if controller.controller_leases.is_empty() {
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

                let mut pairing = self.fallback.upsert_pairing(pairing).await;
                let mut pending_status = self
                    .fallback
                    .take_pending_transport_status(child_session_id)
                    .await;
                // If the controller advertised transport readiness before pairings existed,
                // reuse its cached status for every child pairing.
                if pending_status.is_none() {
                    if let Some(controller_status) = self
                        .fallback
                        .take_pending_transport_status(controller_session_id)
                        .await
                    {
                        pending_status = Some(controller_status.clone());
                        // Reinsert so other children can also pick it up.
                        self.fallback
                            .record_pending_transport_status(
                                controller_session_id,
                                controller_status,
                            )
                            .await;
                    }
                }
                if let Some(status) = pending_status.clone() {
                    pairing.transport_status = Some(status.clone());
                }
                {
                    let mut sessions = self.fallback.sessions.write().await;
                    if let Some(record) = sessions.get_mut(controller_session_id) {
                        let token = record.first_lease_token();
                        record.append_event(
                            ControllerEventType::PairingAdded,
                            token,
                            prompt_template,
                        );
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
                if let Some(status) = pending_status {
                    self.update_pairing_transport_status(child_session_id, status)
                        .await;
                }

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

                let lease = self
                    .fetch_any_active_lease(pool, controller_identifiers.session_id)
                    .await?
                    .ok_or(StateError::ControllerLeaseRequired)?;

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

                let mut pairing = row.into_pairing(&controller_identifiers.private_beach_id);
                let pending_status = self
                    .fallback
                    .take_pending_transport_status(child_session_id)
                    .await;
                if let Some(status) = pending_status.clone() {
                    pairing.transport_status = Some(status.clone());
                }
                let pairing = self.fallback.upsert_pairing(pairing).await;
                self.publish_pairing_event(
                    controller_session_id,
                    child_session_id,
                    action,
                    Some(pairing.clone()),
                )
                .await;
                if let Some(status) = pending_status {
                    self.update_pairing_transport_status(child_session_id, status)
                        .await;
                }
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
        if let Some(account_id) = actor_account_id.as_ref() {
            self.ensure_controller_account_active(
                account_id,
                "controller_pairing_delete",
                Some(controller_session_id),
            )
            .await?;
        }
        match &self.backend {
            Backend::Memory => {
                let controller = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions.get(controller_session_id).cloned()
                }
                .ok_or(StateError::SessionNotFound)?;

                if controller.controller_leases.is_empty() {
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
                        let token = record.first_lease_token();
                        record.append_event(ControllerEventType::PairingRemoved, token, None);
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

                let lease = self
                    .fetch_any_active_lease(pool, controller_identifiers.session_id)
                    .await?
                    .ok_or(StateError::ControllerLeaseRequired)?;

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
        let backend_label = match &self.backend {
            Backend::Memory => "memory",
            Backend::Postgres(_) => "postgres",
        };
        let lease_timer = Instant::now();
        let requester_uuid = requester;
        if let Some(account_id) = requester_uuid.as_ref() {
            self.ensure_controller_account_active(
                account_id,
                "acquire_controller",
                Some(session_id),
            )
            .await?;
        }

        let response = match &self.backend {
            Backend::Memory => {
                self.acquire_controller_memory(session_id, ttl_override, reason.clone())
                    .await?
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let ttl = ttl_override.unwrap_or(DEFAULT_LEASE_TTL_MS).max(1_000);
                let expires_at = Utc::now() + Duration::milliseconds(ttl as i64);
                let requester_uuid = requester_uuid;

                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                if let Some(account) = requester_uuid {
                    self.set_account_context_tx(&mut tx, Some(&account)).await?;
                } else {
                    self.set_account_context_tx(&mut tx, None).await?;
                }

                let existing = self
                    .fetch_active_lease_for_actor_tx(
                        &mut tx,
                        identifiers.session_id,
                        requester_uuid,
                        reason.as_deref(),
                    )
                    .await?;

                let (controller_token, lease_account_id) = if let Some(lease) = existing {
                    sqlx::query(
                        r#"
                        UPDATE controller_lease
                        SET expires_at = $1, issued_at = NOW()
                        WHERE id = $2
                        "#,
                    )
                    .bind(expires_at)
                    .bind(lease.id)
                    .execute(tx.as_mut())
                    .await?;
                    (lease.id, lease.controller_account_id)
                } else {
                    // Attempt to create a new lease for this (account, reason).
                    // In some dev environments, a legacy unique constraint on (session_id)
                    // may still exist. If the INSERT fails with a unique violation, fall
                    // back to returning any active lease for the session so callers can
                    // proceed (semantically equivalent to idempotent renew).
                    let new_token = Uuid::new_v4();
                    let insert_result = sqlx::query(
                        r#"
                        INSERT INTO controller_lease (
                            id, session_id, controller_account_id, issued_by_account_id,
                            reason, issued_at, expires_at, revoked_at
                        )
                        VALUES ($1, $2, $3, $3, $4, NOW(), $5, NULL)
                        "#,
                    )
                    .bind(new_token)
                    .bind(identifiers.session_id)
                    .bind(requester_uuid)
                    .bind(reason.clone())
                    .bind(expires_at)
                    .execute(tx.as_mut())
                    .await;
                    match insert_result {
                        Ok(_) => (new_token, requester_uuid),
                        Err(err) => {
                            // 23505 = unique_violation
                            let is_unique = matches!(&err, sqlx::Error::Database(db)
                                if db.code().as_deref() == Some("23505"));
                            if is_unique {
                                // Return the most recent active lease for this session.
                                let active = self
                                    .list_active_leases_tx(&mut tx, identifiers.session_id)
                                    .await?;
                                if let Some(lease) = active.first() {
                                    (lease.id, lease.controller_account_id)
                                } else {
                                    // No active leases despite unique violation; bubble up.
                                    return Err(err.into());
                                }
                            } else {
                                return Err(err.into());
                            }
                        }
                    }
                };

                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_acquired",
                    Some(controller_token),
                    lease_account_id,
                    requester_uuid,
                    reason.clone(),
                )
                .await?;

                let active_leases = self
                    .list_active_leases_tx(&mut tx, identifiers.session_id)
                    .await?;

                tx.commit().await?;

                self.fallback
                    .acknowledge_controller(
                        session_id,
                        controller_token.to_string(),
                        ttl,
                        lease_account_id.map(|u| u.to_string()),
                        requester_uuid.map(|u| u.to_string()),
                        reason.clone(),
                    )
                    .await;

                // Emit a controller event on the SSE channel
                self.publish(
                    session_id,
                    StreamEvent::ControllerEvent(ControllerEvent {
                        id: Uuid::new_v4().to_string(),
                        event_type: ControllerEventType::LeaseAcquired,
                        controller_token: Some(controller_token.to_string()),
                        timestamp_ms: now_ms(),
                        reason: reason.clone(),
                        controller_account_id: lease_account_id.map(|u| u.to_string()),
                        issued_by_account_id: requester_uuid.map(|u| u.to_string()),
                    }),
                )
                .await;

                self.log_controller_leases(
                    "lease_update",
                    session_id,
                    &identifiers.private_beach_id.to_string(),
                    &active_leases,
                );

                ControllerLeaseResponse {
                    controller_token: controller_token.to_string(),
                    expires_at_ms: expires_at.timestamp_millis(),
                }
            }
        };
        self.refresh_idle_publish_token_hint(session_id).await?;
        if should_log_custom_event(
            "controller_lease_issued",
            session_id,
            StdDuration::from_secs(15),
        ) {
            trace!(
                target = "controller.actions",
                session_id = %session_id,
                reason = reason.as_deref().unwrap_or(""),
                backend = backend_label,
                wait_ms = lease_timer.elapsed().as_millis() as u64,
                "controller lease issued"
            );
        }

        Ok(response)
    }

    pub async fn release_controller(
        &self,
        session_id: &str,
        controller_token: &str,
        actor_account_id: Option<Uuid>,
    ) -> Result<(), StateError> {
        if let Some(account_id) = actor_account_id.as_ref() {
            self.ensure_controller_account_active(
                account_id,
                "release_controller",
                Some(session_id),
            )
            .await?;
        }
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
                self.fallback
                    .clear_controller(session_id, Some(controller_token))
                    .await;
                if !self.fallback.has_active_leases(session_id).await {
                    self.clear_idle_publish_token_hint(session_id).await?;
                }

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
                let private_beach_id_str = identifiers.private_beach_id.to_string();
                let session_uuid_str = session_uuid.to_string();
                let action_count = actions.len();
                let readiness_override = self
                    .fallback
                    .session_readiness_snapshot(&session_uuid_str)
                    .await;
                let lease = match self
                    .fetch_active_lease_for_token(pool, identifiers.session_id, token_uuid)
                    .await
                {
                    Ok(lease) => lease,
                    Err(StateError::ControllerMismatch) if self.controller_strict_gating() => {
                        return Err(self
                            .controller_command_rejection_with_snapshot(
                                &private_beach_id_str,
                                &session_uuid_str,
                                controller_token,
                                None,
                                actor_account_id,
                                ControllerCommandDropReason::MissingLease,
                                None,
                            )
                            .await);
                    }
                    Err(err) => return Err(err),
                };
                if self.controller_strict_gating() {
                    self.enforce_controller_gate(
                        &private_beach_id_str,
                        &session_uuid_str,
                        controller_token,
                        Some(lease.id),
                        actor_account_id,
                        readiness_override.clone(),
                    )
                    .await?;
                }
                let active_leases = self
                    .list_active_leases(pool, identifiers.session_id)
                    .await?;
                self.log_controller_leases(
                    "queue_actions_validate",
                    session_id,
                    &identifiers.private_beach_id.to_string(),
                    &active_leases,
                );

                let trace_context = self
                    .build_agent_trace_context(pool, &identifiers, &actions)
                    .await;

                let queue_depth = self
                    .pending_actions_count(&private_beach_id_str, &session_uuid_str)
                    .await?;
                let projected_depth = queue_depth.saturating_add(actions.len());
                if projected_depth > MAX_PENDING_ACTIONS_PER_SESSION {
                    if should_log_queue_event(QueueLogKind::Overflow, &session_uuid_str) {
                        warn!(
                            target = "controller.delivery",
                            session_id = %session_uuid,
                            private_beach_id = %private_beach_id_str,
                            queue_depth,
                            attempted = actions.len(),
                            limit = MAX_PENDING_ACTIONS_PER_SESSION,
                            "controller action queue over limit; throttling producer"
                        );
                    }
                    return Err(StateError::ActionQueueFull {
                        session_id: session_uuid_str.clone(),
                        private_beach_id: private_beach_id_str,
                        depth: queue_depth,
                        limit: MAX_PENDING_ACTIONS_PER_SESSION,
                    });
                }
                debug!(
                    target = "controller.delivery",
                    session_id = %session_uuid,
                    private_beach_id = %identifiers.private_beach_id,
                    action_count = actions.len(),
                    transport = "http_fallback",
                    queue_depth,
                    "queuing actions via Redis fallback"
                );

                self.enqueue_actions_redis(
                    &private_beach_id_str,
                    &session_uuid_str,
                    actions.clone(),
                )
                .await?;

                // Metrics: enqueue count and queue depth gauge
                let label0 = private_beach_id_str.clone();
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
                self.emit_connector_events_for_child(
                    session_id,
                    "connector.action:queued",
                    DevtoolsEventLevel::Info,
                    Some(serde_json::json!({
                        "count": action_count,
                        "transport": "http_fallback",
                    })),
                )
                .await;
                Ok(())
            }
        }
    }

    pub async fn poll_actions(&self, session_id: &str) -> Result<Vec<ActionCommand>, StateError> {
        self.mark_session_http_ready(session_id).await;
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

                let active_lease = self
                    .fetch_any_active_lease(pool, identifiers.session_id)
                    .await?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                if let Some(lease) = active_lease {
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
                            issued_by_account_id: lease.issued_by_account_id.map(|u| u.to_string()),
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

                if !actions.is_empty() {
                    debug!(
                        target = "controller.delivery",
                        session_id = %session_uuid,
                        private_beach_id = %identifiers.private_beach_id,
                        action_count = actions.len(),
                        "polled actions for delivery to host"
                    );
                }

                Ok(actions)
            }
        }
    }

    pub async fn pending_actions_depth(&self, session_id: &str) -> Result<usize, StateError> {
        match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                Ok(sessions
                    .get(session_id)
                    .map(|record| record.pending_actions.len())
                    .unwrap_or(0))
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                self.pending_actions_count(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                )
                .await
            }
        }
    }

    pub async fn validate_controller_consumer_token(
        &self,
        session_id: &str,
        controller_token: &str,
    ) -> Result<Option<Uuid>, StateError> {
        match &self.backend {
            Backend::Memory => {
                self.validate_controller_consumer_token_memory(session_id, controller_token)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let token_uuid = parse_uuid(controller_token, "controller_token")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let mut tx = pool.begin().await?;
                self.set_rls_context_tx(&mut tx, &identifiers.private_beach_id)
                    .await?;
                let lease: Option<LeaseRow> = sqlx::query_as(
                    r#"
                    SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
                    FROM controller_lease
                    WHERE session_id = $1 AND id = $2 AND revoked_at IS NULL
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(token_uuid)
                .fetch_optional(tx.as_mut())
                .await?;

                let lease = match lease {
                    Some(row) => row,
                    None => return Err(StateError::ControllerMismatch),
                };

                let new_expiry = Utc::now() + Duration::milliseconds(DEFAULT_LEASE_TTL_MS as i64);
                sqlx::query(
                    r#"
                    UPDATE controller_lease
                    SET expires_at = $1
                    WHERE id = $2
                    "#,
                )
                .bind(new_expiry)
                .bind(lease.id)
                .execute(tx.as_mut())
                .await?;
                tx.commit().await?;

                Ok(lease.controller_account_id)
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
                    if !acks.is_empty()
                        && should_log_custom_event(
                            "controller_acks",
                            session_id,
                            StdDuration::from_secs(10),
                        )
                    {
                        let ok_count = acks
                            .iter()
                            .filter(|ack| matches!(ack.status, AckStatus::Ok))
                            .count();
                        let error_count = acks.len().saturating_sub(ok_count);
                        info!(
                            target = "controller.delivery",
                            session_id = %session_id,
                            via_fast_path,
                            total = acks.len(),
                            ok = ok_count,
                            errors = error_count,
                            "controller acks received"
                        );
                    }
                    let mut status = if via_fast_path {
                        PairingTransportStatus::webrtc(event_time)
                    } else {
                        PairingTransportStatus::http_fallback(event_time, None)
                    };
                    status = status.with_latency(latency);
                    if let Some(err) = error_message {
                        status = status.with_error(Some(err));
                    }
                    self.update_pairing_transport_status(session_id, status.clone())
                        .await;
                    let ok_count = acks
                        .iter()
                        .filter(|ack| matches!(ack.status, AckStatus::Ok))
                        .count();
                    let error_count = acks.len().saturating_sub(ok_count);
                    debug!(
                        target = "controller.delivery",
                        session_id,
                        ack_count = acks.len(),
                        via_fast_path,
                        "controller acks recorded (memory backend)"
                    );
                    self.emit_connector_events_for_child(
                        session_id,
                        "connector.action:ack",
                        if error_count > 0 {
                            DevtoolsEventLevel::Warn
                        } else {
                            DevtoolsEventLevel::Info
                        },
                        Some(serde_json::json!({
                            "count": acks.len(),
                            "ok": ok_count,
                            "errors": error_count,
                            "viaFastPath": via_fast_path,
                            "latencyMs": latency,
                            "status": status.transport,
                        })),
                    )
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
                        PairingTransportStatus::webrtc(event_time)
                    } else {
                        PairingTransportStatus::http_fallback(event_time, None)
                    };
                    status = status.with_latency(latency);
                    if let Some(err) = error_message {
                        status = status.with_error(Some(err));
                    }
                    let status_snapshot = status.clone();
                    self.update_pairing_transport_status(&session_uuid_str, status)
                        .await;
                    debug!(
                        target = "controller.delivery",
                        session_id = %session_uuid,
                        private_beach_id = %identifiers.private_beach_id,
                        ack_count = acks.len(),
                        via_fast_path,
                        "controller acks persisted"
                    );
                    let ok_count = acks
                        .iter()
                        .filter(|ack| matches!(ack.status, AckStatus::Ok))
                        .count();
                    let error_count = acks.len().saturating_sub(ok_count);
                    self.emit_connector_events_for_child(
                        session_id,
                        "connector.action:ack",
                        if error_count > 0 {
                            DevtoolsEventLevel::Warn
                        } else {
                            DevtoolsEventLevel::Info
                        },
                        Some(serde_json::json!({
                            "count": acks.len(),
                            "ok": ok_count,
                            "errors": error_count,
                            "viaFastPath": via_fast_path,
                            "latencyMs": latency,
                            "status": status_snapshot.transport,
                        })),
                    )
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
        _via_fast_path: bool,
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
                let diff_sequence = diff_clone.sequence;
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
                self.state_keepalive
                    .schedule(self.clone(), session_id.to_string(), diff_sequence)
                    .await;
                Ok(())
            }
        }
    }

    pub async fn cleanup_stale_sessions(&self) {
        let active_ids: HashSet<String> = {
            let controller = self.controller_workers.read().await;
            let viewer = self.viewer_workers.read().await;
            controller.keys().chain(viewer.keys()).cloned().collect()
        };

        if active_ids.is_empty() {
            return;
        }

        trace!(
            target = "private_beach.sessions",
            active_workers = active_ids.len(),
            idle_threshold_secs = STALE_SESSION_MAX_IDLE.as_secs(),
            "running stale session sweep"
        );

        let stale_candidates: Vec<String> = match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                let now = Instant::now();
                sessions
                    .iter()
                    .filter_map(|(id, record)| {
                        if let Some(ts) = record.last_health_at {
                            if now.duration_since(ts) > STALE_SESSION_MAX_IDLE {
                                return Some(id.clone());
                            }
                        }
                        None
                    })
                    .collect()
            }
            Backend::Postgres(pool) => {
                let cutoff_secs = STALE_SESSION_MAX_IDLE.as_secs() as f64;
                match sqlx::query(
                    r#"
                    SELECT s.origin_session_id::text AS session_id
                    FROM session s
                    LEFT JOIN session_runtime r ON r.session_id = s.id
                    WHERE
                        (r.last_health_at IS NOT NULL AND r.last_health_at < NOW() - ($1 * INTERVAL '1 second'))
                        OR
                        (r.last_health_at IS NULL AND s.created_at < NOW() - ($1 * INTERVAL '1 second'))
                    "#,
                )
                .bind(cutoff_secs)
                .fetch_all(pool)
                .await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .filter_map(|row| row.try_get::<String, _>("session_id").ok())
                        .collect(),
                    Err(err) => {
                        warn!(
                            target = "private_beach.sessions",
                            error = %err,
                            "failed to query stale sessions"
                        );
                        Vec::new()
                    }
                }
            }
        };

        let mut stale_ids: Vec<String> = stale_candidates
            .into_iter()
            .filter(|id| active_ids.contains(id))
            .collect();
        stale_ids.sort();
        stale_ids.dedup();

        for session_id in &stale_ids {
            self.stop_session_workers(session_id, "stale_session_timeout")
                .await;
            self.clear_session_stream(session_id).await;
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
        if let Some(existing) = workers.get(session_id) {
            if !existing.handle.is_finished() {
                // A viewer worker is already running; keep it alive and avoid spawning another.
                return Ok(());
            }
        }

        let viewer_peer_id = workers
            .get(session_id)
            .map(|worker| worker.peer_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        if let Some(existing) = workers.remove(session_id) {
            existing.cancel.cancel();
            let handle = existing.handle;
            tokio::spawn(async move {
                let _ = handle.await;
            });
        }

        let cancel = CancellationToken::new();
        let state_clone = self.clone();
        let session_id_owned = session_id.to_string();
        let private_beach_id_owned = private_beach_id.clone();
        let join_code_owned = join_code.clone();
        let base_url_owned = base_url.clone();
        let cancel_clone = cancel.clone();
        let viewer_peer_id_owned = viewer_peer_id.clone();
        let handle = tokio::spawn(async move {
            run_viewer_worker(
                state_clone,
                session_id_owned,
                private_beach_id_owned,
                join_code_owned,
                base_url_owned,
                viewer_peer_id_owned,
                cancel_clone,
            )
            .await;
        });
        workers.insert(
            session_id.to_string(),
            ViewerWorker {
                handle,
                cancel,
                peer_id: viewer_peer_id,
            },
        );
        Ok(())
    }

    async fn record_timeline_event(
        &self,
        private_beach_id: &str,
        session_id: &str,
        peer_id: &str,
        event: &str,
        detail: serde_json::Value,
    ) {
        if !timeline_logging_enabled() {
            return;
        }
        let Some(client) = &self.redis else {
            return;
        };
        let entry = json!({
            "ts_ms": Utc::now().timestamp_millis(),
            "event": event,
            "detail": detail,
        });
        let key = redis_timeline_key(private_beach_id, session_id, peer_id);
        let mut conn = match client.get_async_connection().await {
            Ok(conn) => conn,
            Err(err) => {
                debug!(
                    target = "private_beach",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    peer_id = %peer_id,
                    error = %err,
                    "timeline redis connection failed"
                );
                return;
            }
        };
        let res: Result<(), redis::RedisError> = redis::pipe()
            .cmd("LPUSH")
            .arg(&key)
            .arg(entry.to_string())
            .cmd("LTRIM")
            .arg(&key)
            .arg(0)
            .arg(200)
            .query_async(&mut conn)
            .await;
        if let Err(err) = res {
            debug!(
                target = "private_beach",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                peer_id = %peer_id,
                error = %err,
                "timeline redis write failed"
            );
        }
    }

    pub async fn spawn_controller_forwarder(&self, session_id: &str) -> Result<(), StateError> {
        let (private_beach_id, join_code) = {
            let sessions = self.fallback.sessions.read().await;
            let record = sessions
                .get(session_id)
                .ok_or(StateError::SessionNotFound)?;
            let passcode = match &record.viewer_passcode {
                Some(code) => code.clone(),
                None => {
                    debug!(
                        target = "controller.forwarder",
                        session_id = %session_id,
                        "viewer passcode not set; skipping controller forwarder"
                    );
                    return Ok(());
                }
            };
            (record.private_beach_id.clone(), passcode)
        };

        let existing_worker = {
            let mut workers = self.controller_workers.write().await;
            if let Some(existing) = workers.get(session_id) {
                if !existing.handle.is_finished() {
                    debug!(
                        target = "controller.forwarder",
                        session_id = %session_id,
                        private_beach_id = %private_beach_id,
                        "controller forwarder already running; skipping spawn"
                    );
                    return Ok(());
                }
            }
            workers.remove(session_id)
        };
        if let Some(existing) = existing_worker {
            existing.cancel.cancel();
            let handle = existing.handle;
            tokio::spawn(async move {
                let _ = handle.await;
            });
        }
        let state_clone = self.clone();
        let session_id_owned = session_id.to_string();
        info!(
            target = "controller.forwarder",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            "starting controller forwarder worker"
        );
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let cancel_task = cancel.clone();
        let handle = tokio::spawn(async move {
            run_controller_forwarder(
                state_clone,
                session_id_owned,
                private_beach_id,
                join_code,
                cancel_task,
            )
            .await;
        });
        let mut workers = self.controller_workers.write().await;
        workers.insert(
            session_id.to_string(),
            ControllerForwarderWorker {
                handle,
                cancel: cancel_clone,
            },
        );
        Ok(())
    }

    async fn stop_session_workers(&self, session_id: &str, reason: &str) {
        let mut removed = false;
        {
            let mut controllers = self.controller_workers.write().await;
            if let Some(existing) = controllers.remove(session_id) {
                existing.cancel.cancel();
                let handle = existing.handle;
                tokio::spawn(async move {
                    let _ = handle.await;
                });
                removed = true;
            }
        }
        {
            let mut viewers = self.viewer_workers.write().await;
            if let Some(existing) = viewers.remove(session_id) {
                existing.cancel.cancel();
                let handle = existing.handle;
                tokio::spawn(async move {
                    let _ = handle.await;
                });
                removed = true;
            }
        }

        if removed {
            let beach_id = {
                let sessions = self.fallback.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|record| record.private_beach_id.clone())
            };
            info!(
                target = "private_beach.sessions",
                session_id = %session_id,
                private_beach_id = beach_id.as_deref().unwrap_or("unknown"),
                reason,
                "session workers stopped"
            );
        }
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
                self.fallback.clear_controller(session_id, None).await;
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
                self.fallback.clear_controller(session_id, None).await;

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
                if event_type.is_some() {
                    sql.push_str(" AND event_type = $2::controller_event_type");
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

    pub async fn account_active(&self, account_id: Uuid) -> Result<bool, StateError> {
        match &self.backend {
            Backend::Memory => Ok(true),
            Backend::Postgres(pool) => {
                let active: Option<(i32,)> =
                    sqlx::query_as("SELECT 1 FROM account WHERE id = $1 AND status = 'active'")
                        .bind(account_id)
                        .fetch_optional(pool)
                        .await?;
                Ok(active.is_some())
            }
        }
    }

    pub async fn session_attached_to_beach(
        &self,
        session_id: &Uuid,
        private_beach_id: &Uuid,
    ) -> Result<bool, StateError> {
        match &self.backend {
            Backend::Memory => Ok(true),
            Backend::Postgres(pool) => {
                let exists: Option<(Uuid,)> = sqlx::query_as(
                    "SELECT id FROM session WHERE id = $1 AND private_beach_id = $2",
                )
                .bind(*session_id)
                .bind(*private_beach_id)
                .fetch_optional(pool)
                .await?;
                Ok(exists.is_some())
            }
        }
    }
}

impl StateKeepaliveManager {
    fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn schedule(&self, state: AppState, session_id: String, sequence: u64) {
        let mut tasks = self.tasks.write().await;
        if let Some(handle) = tasks.remove(&session_id) {
            handle.cancel();
        }
        let (tx, mut rx) = oneshot::channel();
        let state_clone = state.clone();
        let session_clone = session_id.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sleep(STATE_STREAM_HEARTBEAT_INTERVAL) => {
                        state_clone.publish_state_heartbeat(&session_clone, sequence).await;
                    }
                    _ = &mut rx => {
                        break;
                    }
                }
            }
        });
        tasks.insert(
            session_id,
            StateHeartbeatHandle {
                cancel: Some(tx),
                handle,
            },
        );
    }

    async fn cancel(&self, session_id: &str) {
        let mut tasks = self.tasks.write().await;
        if let Some(handle) = tasks.remove(session_id) {
            handle.cancel();
        }
    }
}

impl StateHeartbeatHandle {
    fn cancel(mut self) {
        if let Some(tx) = self.cancel.take() {
            let _ = tx.send(());
        }
        self.handle.abort();
    }
}

fn parse_redis_action_stream(value: redis::Value) -> Result<Vec<ActionCommand>, StateError> {
    let mut actions = Vec::new();
    if let redis::Value::Bulk(streams) = value {
        let mut idx = 0;
        while idx + 1 < streams.len() {
            if let redis::Value::Bulk(entries) = &streams[idx + 1] {
                actions.extend(decode_action_entries(entries)?);
            }
            idx += 2;
        }
    }
    Ok(actions)
}

fn parse_redis_action_entries(value: redis::Value) -> Result<Vec<ActionCommand>, StateError> {
    if let redis::Value::Bulk(entries) = value {
        return decode_action_entries(&entries);
    }
    Ok(Vec::new())
}

fn decode_action_entries(entries: &[redis::Value]) -> Result<Vec<ActionCommand>, StateError> {
    let mut actions = Vec::new();
    for entry in entries {
        if let redis::Value::Bulk(entry_parts) = entry {
            if entry_parts.len() < 2 {
                continue;
            }
            if let redis::Value::Bulk(fields) = &entry_parts[1] {
                let mut field_idx = 0;
                while field_idx + 1 < fields.len() {
                    let field_name = redis::from_redis_value::<String>(&fields[field_idx])?;
                    if field_name == "payload" {
                        let payload_str: String = redis::from_redis_value(&fields[field_idx + 1])?;
                        let action: ActionCommand = serde_json::from_str(&payload_str)?;
                        actions.push(action);
                    }
                    field_idx += 2;
                }
            }
        }
    }
    Ok(actions)
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
            pending_transport: RwLock::new(HashMap::new()),
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

    async fn record_pending_transport_status(
        &self,
        child_session_id: &str,
        status: PairingTransportStatus,
    ) {
        let mut pending = self.pending_transport.write().await;
        pending.insert(child_session_id.to_string(), status);
    }

    async fn take_pending_transport_status(
        &self,
        child_session_id: &str,
    ) -> Option<PairingTransportStatus> {
        let mut pending = self.pending_transport.write().await;
        pending.remove(child_session_id)
    }

    async fn peek_pending_transport_status(
        &self,
        child_session_id: &str,
    ) -> Option<PairingTransportStatus> {
        let pending = self.pending_transport.read().await;
        pending.get(child_session_id).cloned()
    }

    async fn mark_attached(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.mark_attached();
        }
    }

    async fn mark_http_ready(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.mark_http_ready();
        }
    }

    async fn transport_mode(&self, session_id: &str) -> Option<TransportMode> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|record| record.transport_mode)
    }

    async fn session_readiness_snapshot(
        &self,
        session_id: &str,
    ) -> Option<SessionReadinessSnapshot> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|record| record.readiness_snapshot())
    }

    async fn ensure_session(
        &self,
        req: &RegisterSessionRequest,
        harness_id: &str,
        controller_lease: Option<(String, ControllerLeaseMemory)>,
        idle_snapshot_interval_ms: Option<u64>,
    ) {
        let mut sessions = self.sessions.write().await;
        let requested_mode = req.transport_mode.unwrap_or(TransportMode::FastPath);
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(
                &req.session_id,
                &req.private_beach_id,
                &req.harness_type,
                idle_snapshot_interval_ms,
                requested_mode,
            )
        });
        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();
        entry.harness_id = harness_id.to_string();
        entry.transport_mode = requested_mode;
        entry.controller_leases.clear();
        if let Some((token, lease)) = controller_lease {
            entry.controller_leases.insert(token, lease);
        }
        entry.viewer_passcode = req.viewer_passcode.clone();
    }

    async fn set_transport_hints(
        &self,
        session_id: &str,
        hints: HashMap<String, serde_json::Value>,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.transport_hints = hints;
        }
    }

    async fn acknowledge_controller(
        &self,
        session_id: &str,
        token: String,
        ttl: u64,
        controller_account_id: Option<String>,
        issued_by_account_id: Option<String>,
        reason: Option<String>,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            let expires_at_ms = now_ms() + ttl as i64;
            record.ensure_lease(
                token.clone(),
                expires_at_ms,
                controller_account_id,
                issued_by_account_id,
                reason,
            );
            record.lease_ttl_ms = ttl;
        }
    }

    async fn clear_controller(&self, session_id: &str, token: Option<&str>) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            if let Some(token) = token {
                record.remove_lease(token);
            } else {
                record.clear_leases();
            }
        }
    }

    async fn has_active_leases(&self, session_id: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|record| record.has_active_leases())
            .unwrap_or(false)
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
            record.last_health_at = Some(Instant::now());
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

    async fn children_for_controller(&self, controller_session_id: &str) -> Option<Vec<String>> {
        let guard = self.pairings.read().await;
        guard.get(controller_session_id).map(|list| {
            list.iter()
                .map(|pairing| pairing.child_session_id.clone())
                .collect()
        })
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
                if pairing
                    .transport_status
                    .as_ref()
                    .map(|existing| existing.semantically_equals(&status))
                    .unwrap_or(false)
                {
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
    fn new(
        session_id: &str,
        private_beach_id: &str,
        harness_type: &HarnessType,
        idle_snapshot_interval_ms: Option<u64>,
        transport_mode: TransportMode,
    ) -> Self {
        let harness_id = Uuid::new_v4().to_string();
        let transport_hints = default_transport_hints(session_id, idle_snapshot_interval_ms);
        Self {
            session_id: session_id.to_string(),
            private_beach_id: private_beach_id.to_string(),
            harness_type: harness_type.clone(),
            capabilities: Vec::new(),
            location_hint: None,
            metadata: None,
            version: "unknown".into(),
            harness_id,
            controller_leases: HashMap::new(),
            viewer_passcode: None,
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            transport_hints,
            state_cache_url: None,
            pending_actions: VecDeque::new(),
            controller_events: Vec::new(),
            last_health: None,
            last_health_at: None,
            last_state: None,
            attached_at_ms: None,
            http_ready_since_ms: None,
            transport_mode,
        }
    }

    fn upsert_controller_auto_attach_hint(
        &mut self,
        hint: &ControllerAutoAttachHint,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(hint)?;
        self.transport_hints
            .insert("controller_auto_attach".into(), value);
        Ok(())
    }

    fn upsert_idle_publish_token_hint(
        &mut self,
        hint: &IdlePublishTokenHint,
    ) -> Result<(), serde_json::Error> {
        inject_idle_publish_hint(&mut self.transport_hints, hint);
        Ok(())
    }

    fn clear_idle_publish_token_hint(&mut self) {
        remove_idle_publish_hint(&mut self.transport_hints);
    }

    fn append_event(
        &mut self,
        event_type: ControllerEventType,
        controller_token: Option<String>,
        reason: Option<String>,
    ) {
        self.controller_events.push(ControllerEvent {
            id: Uuid::new_v4().to_string(),
            event_type,
            controller_token,
            timestamp_ms: now_ms(),
            reason,
            controller_account_id: None,
            issued_by_account_id: None,
        });
    }

    fn lease(&self, token: &str) -> Option<&ControllerLeaseMemory> {
        self.controller_leases.get(token)
    }

    fn lease_mut(&mut self, token: &str) -> Option<&mut ControllerLeaseMemory> {
        self.controller_leases.get_mut(token)
    }

    fn ensure_lease(
        &mut self,
        token: String,
        expires_at_ms: i64,
        controller_account_id: Option<String>,
        issued_by_account_id: Option<String>,
        reason: Option<String>,
    ) {
        self.controller_leases.insert(
            token,
            ControllerLeaseMemory {
                expires_at_ms,
                controller_account_id,
                issued_by_account_id,
                reason,
            },
        );
    }

    fn remove_lease(&mut self, token: &str) -> bool {
        self.controller_leases.remove(token).is_some()
    }

    fn clear_leases(&mut self) {
        self.controller_leases.clear();
    }

    fn has_active_leases(&self) -> bool {
        !self.controller_leases.is_empty()
    }

    fn first_lease_token(&self) -> Option<String> {
        self.controller_leases.keys().next().cloned()
    }

    fn find_lease_mut_by_reason(
        &mut self,
        reason: Option<&str>,
    ) -> Option<(&String, &mut ControllerLeaseMemory)> {
        self.controller_leases
            .iter_mut()
            .find(|(_, lease)| lease.reason.as_deref() == reason)
    }

    fn mark_attached(&mut self) {
        self.attached_at_ms = Some(now_ms());
    }

    fn mark_http_ready(&mut self) {
        self.http_ready_since_ms = Some(now_ms());
    }

    fn readiness_snapshot(&self) -> SessionReadinessSnapshot {
        SessionReadinessSnapshot {
            attached_at_ms: self.attached_at_ms,
            http_ready_since_ms: self.http_ready_since_ms,
            last_health_at: self.last_health_at,
        }
    }
}

impl SessionReadinessSnapshot {
    fn attached(&self) -> bool {
        self.attached_at_ms.is_some()
    }

    fn http_ready(&self) -> bool {
        self.http_ready_since_ms.is_some()
    }

    fn child_online(&self) -> bool {
        match self.last_health_at {
            Some(last) => last.elapsed() < STALE_SESSION_MAX_IDLE,
            None => true,
        }
    }

    fn ever_reported_health(&self) -> bool {
        self.last_health_at.is_some()
    }

    fn attach_age_seconds(&self) -> Option<f64> {
        self.attached_at_ms.map(|ts| {
            let elapsed_ms = now_ms().saturating_sub(ts);
            (elapsed_ms.max(0) as f64) / 1000.0
        })
    }
}

impl SessionSummary {
    fn from_record(record: &SessionRecord) -> Self {
        let (controller_token, controller_expires_at_ms) = record
            .controller_leases
            .iter()
            .next()
            .map(|(token, lease)| (Some(token.clone()), Some(lease.expires_at_ms)))
            .unwrap_or((None, None));
        Self {
            session_id: record.session_id.clone(),
            private_beach_id: record.private_beach_id.clone(),
            harness_type: record.harness_type.clone(),
            capabilities: record.capabilities.clone(),
            location_hint: record.location_hint.clone(),
            metadata: record.metadata.clone(),
            version: record.version.clone(),
            harness_id: record.harness_id.clone(),
            controller_token,
            controller_expires_at_ms,
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

fn truncate_uuid(id: &Uuid) -> String {
    let full = id.to_string();
    full.split('-').next().unwrap_or(&full).to_string()
}

fn redact_controller_token(token: &str) -> String {
    match Uuid::parse_str(token) {
        Ok(uuid) => truncate_uuid(&uuid),
        Err(_) => token.chars().take(8).collect(),
    }
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

fn default_transport_hints(
    session_id: &str,
    idle_snapshot_interval_ms: Option<u64>,
) -> HashMap<String, serde_json::Value> {
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
    if let Some(interval) = idle_snapshot_interval_ms {
        hints.insert(
            "idle_snapshot".into(),
            serde_json::json!({
                "interval_ms": interval,
                "mode": "terminal_full"
            }),
        );
    }
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

fn inject_idle_publish_hint(
    hints: &mut HashMap<String, serde_json::Value>,
    hint: &IdlePublishTokenHint,
) {
    let value = hint.as_value();
    hints.insert(IDLE_PUBLISH_TOKEN_HINT_KEY.into(), value.clone());
    if let Some(obj) = hints
        .get_mut("idle_snapshot")
        .and_then(|v| v.as_object_mut())
    {
        obj.insert("publish_token".into(), value);
    }
}

fn remove_idle_publish_hint(hints: &mut HashMap<String, serde_json::Value>) {
    hints.remove(IDLE_PUBLISH_TOKEN_HINT_KEY);
    if let Some(obj) = hints
        .get_mut("idle_snapshot")
        .and_then(|v| v.as_object_mut())
    {
        obj.remove("publish_token");
    }
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

fn redis_timeline_key(private_beach_id: &str, session_id: &str, peer_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:timeline:{peer_id}")
}

fn timeline_logging_enabled() -> bool {
    matches!(
        std::env::var("BEACH_TIMELINE_REDIS").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("on")
    )
}

fn parse_pending_entries(value: redis::Value) -> Vec<String> {
    if let redis::Value::Bulk(entries) = value {
        entries
            .into_iter()
            .filter_map(|entry| match entry {
                redis::Value::Bulk(fields) if !fields.is_empty() => {
                    if let redis::Value::Data(id_bytes) = &fields[0] {
                        String::from_utf8(id_bytes.clone()).ok()
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    }
}

impl AppState {
    async fn register_session_memory(
        &self,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let transport_mode = req.transport_mode.unwrap_or(TransportMode::FastPath);
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(
                &req.session_id,
                &req.private_beach_id,
                &req.harness_type,
                self.idle_snapshot_interval_ms,
                transport_mode,
            )
        });

        entry.transport_mode = transport_mode;
        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();
        entry.viewer_passcode = req.viewer_passcode.clone();
        self.ensure_controller_auto_attach_hint(entry, &req.private_beach_id)?;
        let publish_hint = IdlePublishTokenHint::from_signed(
            &self.publish_tokens.sign_for_session(&req.session_id),
        );
        entry.upsert_idle_publish_token_hint(&publish_hint)?;

        if entry.controller_leases.is_empty() {
            let token = Uuid::new_v4().to_string();
            let expires_at_ms = now_ms() + entry.lease_ttl_ms as i64;
            entry.ensure_lease(token, expires_at_ms, None, None, None);
        }

        // Memory backend bypasses controller gating to keep test harnesses simple by
        // treating freshly registered sessions as attached + HTTP-ready.
        entry.mark_attached();
        entry.mark_http_ready();

        let controller_token = entry.first_lease_token();
        entry.append_event(
            ControllerEventType::Registered,
            controller_token.clone(),
            None,
        );

        let response = RegisterSessionResponse {
            harness_id: entry.harness_id.clone(),
            controller_token,
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
        let transport_hints = self.prepare_transport_hints_for_registration(&req).await?;
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
            UPDATE controller_lease
            SET revoked_at = NOW(), expires_at = NOW()
            WHERE session_id = $1
            "#,
        )
        .bind(db_session_id)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO controller_lease (
                id, session_id, controller_account_id, issued_by_account_id,
                reason, issued_at, expires_at, revoked_at
            )
            VALUES ($1, $2, NULL, NULL, $3, NOW(), $4, NULL)
            "#,
        )
        .bind(controller_token)
        .bind(db_session_id)
        .bind(Some("session_register"))
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

        let fallback_lease = ControllerLeaseMemory {
            expires_at_ms: expires_at.timestamp_millis(),
            controller_account_id: None,
            issued_by_account_id: None,
            reason: None,
        };
        self.fallback
            .ensure_session(
                &req,
                &harness_id.to_string(),
                Some((controller_token.to_string(), fallback_lease)),
                self.idle_snapshot_interval_ms,
            )
            .await;
        self.fallback
            .set_transport_hints(&req.session_id, transport_hints.clone())
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
        let expires_at_ms = now_ms() + ttl as i64;
        let token = if let Some((token, lease)) = record.find_lease_mut_by_reason(reason.as_deref())
        {
            lease.expires_at_ms = expires_at_ms;
            token.clone()
        } else {
            let token = Uuid::new_v4().to_string();
            record.ensure_lease(token.clone(), expires_at_ms, None, None, reason.clone());
            token
        };
        record.append_event(
            ControllerEventType::LeaseAcquired,
            Some(token.clone()),
            reason,
        );
        self.log_memory_leases("lease_update", record);

        Ok(ControllerLeaseResponse {
            controller_token: token,
            expires_at_ms,
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
        if !record.remove_lease(controller_token) {
            return Err(StateError::ControllerMismatch);
        }
        let token_string = controller_token.to_string();
        record.append_event(
            ControllerEventType::LeaseReleased,
            Some(token_string.clone()),
            None,
        );
        let has_active = record.has_active_leases();
        self.log_memory_leases("lease_release", record);
        drop(sessions);
        if !has_active {
            self.clear_idle_publish_token_hint(session_id).await?;
        }
        self.publish(
            session_id,
            StreamEvent::ControllerEvent(ControllerEvent {
                id: Uuid::new_v4().to_string(),
                event_type: ControllerEventType::LeaseReleased,
                controller_token: Some(token_string),
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
        enum LeaseState {
            Valid {
                private_beach_id: String,
                readiness: Option<SessionReadinessSnapshot>,
                http_ready: bool,
            },
            Missing {
                private_beach_id: String,
                readiness: Option<SessionReadinessSnapshot>,
            },
        }

        let lease_state = {
            let mut sessions = self.fallback.sessions.write().await;
            let record = sessions
                .get_mut(session_id)
                .ok_or(StateError::SessionNotFound)?;
            let snapshot = record.readiness_snapshot();
            let readiness = if self.controller_strict_gating() {
                Some(snapshot.clone())
            } else {
                None
            };
            let http_ready = snapshot.http_ready();
            let private_beach_id = record.private_beach_id.clone();
            match record.lease(controller_token).cloned() {
                Some(lease) if lease.expires_at_ms > now_ms() => LeaseState::Valid {
                    private_beach_id,
                    readiness,
                    http_ready,
                },
                _ => LeaseState::Missing {
                    private_beach_id,
                    readiness,
                },
            }
        };

        let (private_beach_id, readiness, http_ready_flag) = match lease_state {
            LeaseState::Valid {
                private_beach_id,
                readiness,
                http_ready,
            } => (private_beach_id, readiness, http_ready),
            LeaseState::Missing {
                private_beach_id,
                readiness,
            } => {
                warn!(
                    target = "controller.leases",
                    private_beach_id = %private_beach_id,
                    session_id = %session_id,
                    controller_token = %redact_controller_token(controller_token),
                    "queue_actions missing lease"
                );
                if self.controller_strict_gating() {
                    return Err(self
                        .controller_command_rejection_with_snapshot(
                            &private_beach_id,
                            session_id,
                            controller_token,
                            None,
                            None,
                            ControllerCommandDropReason::MissingLease,
                            readiness,
                        )
                        .await);
                }
                return Err(StateError::ControllerMismatch);
            }
        };

        let readiness_snapshot = readiness.clone();
        let _http_ready = readiness_snapshot
            .as_ref()
            .map(|snapshot| snapshot.http_ready())
            .unwrap_or(http_ready_flag);
        if self.controller_strict_gating() {
            self.enforce_controller_gate(
                &private_beach_id,
                session_id,
                controller_token,
                None,
                None,
                readiness,
            )
            .await?;
        }

        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let action_count = actions.len();
        for action in actions {
            record.pending_actions.push_back(action);
        }
        self.log_memory_leases("queue_actions_validate", record);
        let token_string = controller_token.to_string();
        record.append_event(
            ControllerEventType::ActionsQueued,
            Some(token_string.clone()),
            None,
        );
        let event_time = now_ms();
        let event = StreamEvent::ControllerEvent(ControllerEvent {
            id: Uuid::new_v4().to_string(),
            event_type: ControllerEventType::ActionsQueued,
            controller_token: Some(token_string),
            timestamp_ms: event_time,
            reason: None,
            controller_account_id: None,
            issued_by_account_id: None,
        });
        drop(sessions);

        // Unified transport uses the primary channel; keep transport status at webrtc so
        // agents don't fall back to HTTP while actions queue.
        self.update_pairing_transport_status(
            session_id,
            PairingTransportStatus::webrtc(event_time),
        )
        .await;

        self.publish(session_id, event).await;
        self.emit_connector_events_for_child(
            session_id,
            "connector.action:queued",
            DevtoolsEventLevel::Info,
            Some(serde_json::json!({
                "count": action_count,
                "transport": "http_fallback",
                "controllerToken": controller_token,
            })),
        )
        .await;
        Ok(())
    }

    async fn validate_controller_consumer_token_memory(
        &self,
        session_id: &str,
        controller_token: &str,
    ) -> Result<Option<Uuid>, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let lease = record
            .lease_mut(controller_token)
            .ok_or(StateError::ControllerMismatch)?;
        let account_uuid = lease
            .controller_account_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok());
        lease.expires_at_ms = now_ms() + DEFAULT_LEASE_TTL_MS as i64;
        Ok(account_uuid)
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
        let ack_token = if !actions.is_empty() {
            Some(record.first_lease_token())
        } else {
            None
        };
        drop(sessions);
        if let Some(token) = ack_token {
            self.publish(
                session_id,
                StreamEvent::ControllerEvent(ControllerEvent {
                    id: Uuid::new_v4().to_string(),
                    event_type: ControllerEventType::ActionsAcked,
                    controller_token: token,
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
        let token = record.first_lease_token();
        record.append_event(ControllerEventType::HealthReported, token, None);
        drop(sessions);
        self.publish(session_id, StreamEvent::Health(heartbeat))
            .await;
        Ok(())
    }

    async fn record_state_memory(
        &self,
        session_id: &str,
        diff: StateDiff,
    ) -> Result<(), StateError> {
        let (private_beach_id, diff_clone, diff_sequence, payload_summary) = {
            let mut sessions = self.fallback.sessions.write().await;
            let record = sessions
                .get_mut(session_id)
                .ok_or(StateError::SessionNotFound)?;
            let diff_clone = diff.clone();
            let diff_sequence = diff_clone.sequence;
            let private_beach_id = record.private_beach_id.clone();
            record.last_state = Some(diff);
            let token = record.first_lease_token();
            record.append_event(ControllerEventType::StateUpdated, token, None);
            let payload_summary = diff_clone
                .payload
                .as_object()
                .map(|object| {
                    let keys = object
                        .keys()
                        .take(6)
                        .map(|key| key.to_string())
                        .collect::<Vec<_>>();
                    let line_count = object
                        .get("lines")
                        .and_then(|value| value.as_array())
                        .map(|rows| rows.len());
                    (keys, line_count)
                })
                .unwrap_or_default();
            drop(sessions);
            (private_beach_id, diff_clone, diff_sequence, payload_summary)
        };
        self.store_state_redis(&private_beach_id, session_id, &diff_clone)
            .await?;
        self.publish(session_id, StreamEvent::State(diff_clone))
            .await;
        self.state_keepalive
            .schedule(self.clone(), session_id.to_string(), diff_sequence)
            .await;
        let (payload_keys, line_count) = payload_summary;
        self.emit_connector_events_for_child(
            session_id,
            "connector.child:update",
            DevtoolsEventLevel::Info,
            Some(serde_json::json!({
                "sequence": diff_sequence,
                "keys": payload_keys,
                "lineCount": line_count,
            })),
        )
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

    async fn fetch_active_lease_for_actor_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
        controller_account_id: Option<Uuid>,
        reason: Option<&str>,
    ) -> Result<Option<LeaseRow>, StateError> {
        let row: Option<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
              AND revoked_at IS NULL
              AND expires_at > NOW()
              AND (controller_account_id IS NOT DISTINCT FROM $2)
              AND (reason IS NOT DISTINCT FROM $3)
            ORDER BY expires_at DESC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .bind(controller_account_id)
        .bind(reason)
        .fetch_optional(tx.as_mut())
        .await?;

        Ok(row)
    }

    async fn fetch_active_lease_for_token(
        &self,
        pool: &PgPool,
        session_id: Uuid,
        token_id: Uuid,
    ) -> Result<LeaseRow, StateError> {
        let row: Option<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
              AND id = $2
            "#,
        )
        .bind(session_id)
        .bind(token_id)
        .fetch_optional(pool)
        .await?;

        match row {
            Some(lease) if is_active_lease(lease.expires_at, lease.revoked_at) => Ok(lease),
            _ => Err(StateError::ControllerMismatch),
        }
    }

    async fn list_active_leases_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
    ) -> Result<Vec<LeaseRow>, StateError> {
        let rows: Vec<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
              AND revoked_at IS NULL
              AND expires_at > NOW()
            ORDER BY expires_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(tx.as_mut())
        .await?;

        Ok(rows)
    }

    async fn list_active_leases(
        &self,
        pool: &PgPool,
        session_id: Uuid,
    ) -> Result<Vec<LeaseRow>, StateError> {
        let rows: Vec<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
              AND revoked_at IS NULL
              AND expires_at > NOW()
            ORDER BY expires_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(pool)
        .await?;

        Ok(rows)
    }

    async fn fetch_any_active_lease(
        &self,
        pool: &PgPool,
        session_id: Uuid,
    ) -> Result<Option<LeaseRow>, StateError> {
        let row: Option<LeaseRow> = sqlx::query_as(
            r#"
            SELECT id, controller_account_id, issued_by_account_id, reason, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
              AND revoked_at IS NULL
              AND expires_at > NOW()
            ORDER BY expires_at DESC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

        Ok(row)
    }

    fn log_controller_leases(
        &self,
        event: &str,
        session_id: &str,
        private_beach_id: &str,
        leases: &[LeaseRow],
    ) {
        if !tracing::enabled!(Level::INFO) {
            return;
        }
        if !should_log_queue_event(QueueLogKind::Validate, session_id) {
            return;
        }
        let now = now_ms();
        let summary: Vec<String> = leases
            .iter()
            .map(|lease| {
                let token = truncate_uuid(&lease.id);
                let account = lease
                    .controller_account_id
                    .as_ref()
                    .map(truncate_uuid)
                    .unwrap_or_else(|| "anon".into());
                let expires_in = lease
                    .expires_at
                    .map(|dt| (dt.timestamp_millis() - now).max(0))
                    .unwrap_or(0);
                let reason = lease.reason.as_deref().unwrap_or("<none>");
                format!("{token}@{account}:expires_in={expires_in}ms reason={reason}")
            })
            .collect();
        info!(
            target = "controller.leases",
            event,
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            active = leases.len(),
            controllers = ?summary
        );
    }

    fn log_memory_leases(&self, event: &str, record: &SessionRecord) {
        if !tracing::enabled!(Level::INFO) {
            return;
        }
        let now = now_ms();
        let summary: Vec<String> = record
            .controller_leases
            .iter()
            .map(|(token, lease)| {
                let account = lease.controller_account_id.as_deref().unwrap_or("anon");
                let issuer = lease.issued_by_account_id.as_deref().unwrap_or("anon");
                let expires_in = (lease.expires_at_ms - now).max(0);
                let reason = lease.reason.as_deref().unwrap_or("<none>");
                format!(
                    "{token}@{account}:issued_by={issuer} expires_in={expires_in}ms reason={reason}"
                )
            })
            .collect();
        info!(
            target = "controller.leases",
            event,
            session_id = %record.session_id,
            private_beach_id = %record.private_beach_id,
            active = record.controller_leases.len(),
            controllers = ?summary
        );
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

    async fn ensure_action_consumer_group(
        &self,
        conn: &mut redis::aio::Connection,
        stream_key: &str,
    ) -> Result<(), StateError> {
        if let Err(err) = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(stream_key)
            .arg(REDIS_ACTION_GROUP)
            .arg("0")
            .arg("MKSTREAM")
            .query_async::<_, ()>(conn)
            .await
        {
            if err.code() != Some("BUSYGROUP") {
                return Err(StateError::Redis(err));
            }
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
            self.ensure_action_consumer_group(&mut conn, &key).await?;

            let value: redis::Value = loop {
                match redis::cmd("XREADGROUP")
                    .arg("GROUP")
                    .arg(REDIS_ACTION_GROUP)
                    .arg(&consumer)
                    .arg("COUNT")
                    .arg(64)
                    .arg("STREAMS")
                    .arg(&key)
                    .arg(">")
                    .query_async(&mut conn)
                    .await
                {
                    Ok(val) => break val,
                    Err(err) if err.code() == Some("NOGROUP") => {
                        self.ensure_action_consumer_group(&mut conn, &key).await?;
                        continue;
                    }
                    Err(err) => return Err(StateError::Redis(err)),
                }
            };

            let mut actions = Vec::new();

            if !matches!(value, redis::Value::Nil) {
                actions = parse_redis_action_stream(value)?;
            }

            if actions.is_empty() {
                let reclaimed = self
                    .reclaim_pending_actions(&mut conn, &key, &consumer)
                    .await?;
                if !reclaimed.is_empty() {
                    let label0 = private_beach_id.to_string();
                    let label1 = session_id.to_string();
                    metrics::REDIS_PENDING_RECLAIMED
                        .with_label_values(&[label0.as_str(), label1.as_str()])
                        .inc_by(reclaimed.len() as u64);
                    debug!(
                        target = "controller.delivery",
                        session_id = %session_id,
                        private_beach_id = %private_beach_id,
                        reclaimed = reclaimed.len(),
                        "reclaimed pending redis controller actions"
                    );
                    return Ok(reclaimed);
                }
            }

            // If Redis reports a non-zero stream length but this consumer
            // didn't receive any new entries, log a sampled diagnostic so
            // we can tell whether messages are stuck in the pending set.
            if actions.is_empty()
                && should_log_custom_event(
                    "redis_drain_empty",
                    session_id,
                    StdDuration::from_secs(15),
                )
            {
                let depth = self
                    .pending_actions_count(private_beach_id, session_id)
                    .await
                    .unwrap_or(0);
                if depth > 0 {
                    let rtc_ready = self.rtc_ready(session_id).await;
                    warn!(
                        target = "controller.delivery",
                        session_id = %session_id,
                        private_beach_id = %private_beach_id,
                        queue_depth = depth,
                        consumer = %consumer,
                        rtc_ready,
                        "drain_actions_redis returned no actions despite non-empty stream"
                    );
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

    async fn reclaim_pending_actions(
        &self,
        conn: &mut redis::aio::Connection,
        key: &str,
        consumer: &str,
    ) -> Result<Vec<ActionCommand>, StateError> {
        let pending_value = redis::cmd("XPENDING")
            .arg(key)
            .arg(REDIS_ACTION_GROUP)
            .arg("-")
            .arg("+")
            .arg(REDIS_PENDING_RECLAIM_MAX as i64)
            .query_async::<_, redis::Value>(conn)
            .await?;
        let entry_ids = parse_pending_entries(pending_value);
        if entry_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut cmd = redis::cmd("XCLAIM");
        cmd.arg(key)
            .arg(REDIS_ACTION_GROUP)
            .arg(consumer)
            .arg(REDIS_PENDING_RECLAIM_MIN_IDLE_MS);
        for entry in entry_ids.into_iter().take(REDIS_PENDING_RECLAIM_MAX) {
            cmd.arg(entry);
        }

        let claimed = cmd.query_async::<_, redis::Value>(conn).await?;
        parse_redis_action_entries(claimed)
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

    pub async fn delete_private_beach(
        &self,
        id_str: &str,
        account: Option<Uuid>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                let session_ids = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions
                        .iter()
                        .filter_map(|(session_id, record)| {
                            if record.private_beach_id == id_str {
                                Some(session_id.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<String>>()
                };
                for session_id in &session_ids {
                    self.stop_session_workers(session_id, "private_beach_deleted")
                        .await;
                    self.clear_session_stream(session_id).await;
                }
                self.purge_private_beach_state(id_str, &session_ids).await;
                info!(
                    target = "private_beach",
                    private_beach_id = %id_str,
                    session_count = session_ids.len(),
                    "private beach deleted (memory backend)"
                );
                Ok(())
            }
            Backend::Postgres(pool) => {
                let id = parse_uuid(id_str, "id")?;
                let private_beach_id = id.to_string();
                let mut tx = pool.begin().await?;
                if let Some(owner) = account {
                    self.set_account_context_tx(&mut tx, Some(&owner)).await?;
                } else {
                    self.set_account_context_tx(&mut tx, None).await?;
                }
                self.set_rls_context_tx(&mut tx, &id).await?;

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

                let session_rows: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT origin_session_id FROM session WHERE private_beach_id = $1",
                )
                .bind(id)
                .fetch_all(tx.as_mut())
                .await?;

                let deleted: Option<Uuid> =
                    sqlx::query_scalar("DELETE FROM private_beach WHERE id = $1 RETURNING id")
                        .bind(id)
                        .fetch_optional(tx.as_mut())
                        .await?;

                if deleted.is_none() {
                    return Err(StateError::PrivateBeachNotFound);
                }

                tx.commit().await?;

                let mut session_ids: Vec<String> = session_rows
                    .into_iter()
                    .map(|row| row.to_string())
                    .collect();
                let fallback_sessions = {
                    let sessions = self.fallback.sessions.read().await;
                    sessions
                        .iter()
                        .filter_map(|(session_id, record)| {
                            if record.private_beach_id == private_beach_id {
                                Some(session_id.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<String>>()
                };
                for id in fallback_sessions {
                    if !session_ids.contains(&id) {
                        session_ids.push(id);
                    }
                }

                for session_id in &session_ids {
                    self.stop_session_workers(session_id, "private_beach_deleted")
                        .await;
                    self.clear_session_stream(session_id).await;
                }

                self.purge_private_beach_state(&private_beach_id, &session_ids)
                    .await;
                info!(
                    target = "private_beach",
                    private_beach_id = %private_beach_id,
                    session_count = session_ids.len(),
                    "private beach deleted"
                );
                Ok(())
            }
        }
    }

    async fn purge_private_beach_state(&self, private_beach_id: &str, session_ids: &[String]) {
        let session_set: HashSet<String> = session_ids.iter().cloned().collect();
        {
            let mut sessions = self.fallback.sessions.write().await;
            sessions.retain(|session_id, record| {
                if record.private_beach_id == private_beach_id {
                    return false;
                }
                !session_set.contains(session_id)
            });
        }
        {
            let mut pairings = self.fallback.pairings.write().await;
            pairings.retain(|controller_id, list| {
                if session_set.contains(controller_id) {
                    return false;
                }
                list.retain(|pairing| !session_set.contains(&pairing.child_session_id));
                !list.is_empty()
            });
        }
        {
            let mut pending = self.fallback.pending_transport.write().await;
            for session_id in &session_set {
                pending.remove(session_id);
            }
        }
        {
            let mut layouts = self.fallback.canvas_layouts.write().await;
            layouts.remove(private_beach_id);
        }
        {
            let mut handshakes = self.controller_handshakes.write().await;
            for session_id in &session_set {
                handshakes.remove(session_id);
            }
        }
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
#[derive(Debug, Clone, FromRow)]
struct LeaseRow {
    id: Uuid,
    controller_account_id: Option<Uuid>,
    issued_by_account_id: Option<Uuid>,
    reason: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

async fn run_viewer_worker(
    state: AppState,
    session_id: String,
    private_beach_id: String,
    join_code: String,
    road_base_url: String,
    viewer_peer_id: String,
    cancel: CancellationToken,
) {
    #[cfg(test)]
    if let Some(override_fn) = test_support::viewer_worker_override() {
        override_fn(
            state,
            session_id,
            private_beach_id,
            join_code,
            road_base_url,
            viewer_peer_id,
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
    let mut last_error: Option<String> = None;
    loop {
        if cancel.is_cancelled() {
            info!(
                target = "private_beach",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                "viewer worker cancelled"
            );
            break;
        }
        if attempts > 0 {
            metrics::MANAGER_VIEWER_RECONNECTS
                .with_label_values(&[label_private.as_str(), label_session.as_str()])
                .inc();
        }
        attempts = attempts.saturating_add(1);
        let ice_env = [
            (
                "BEACH_GATE_TURN_URLS",
                env::var("BEACH_GATE_TURN_URLS").ok(),
            ),
            (
                "BEACH_TURN_EXTERNAL_IP",
                env::var("BEACH_TURN_EXTERNAL_IP").ok(),
            ),
            ("BEACH_ICE_PUBLIC_IP", env::var("BEACH_ICE_PUBLIC_IP").ok()),
            (
                "BEACH_ICE_PUBLIC_HOST",
                env::var("BEACH_ICE_PUBLIC_HOST").ok(),
            ),
        ];
        info!(
            target = "private_beach",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            viewer_peer_id = %viewer_peer_id,
            attempt = attempts,
            ice_env = ?ice_env,
            last_error = last_error.as_deref().unwrap_or("none"),
            "manager viewer starting connect attempt"
        );
        match viewer_connect_once(
            &state,
            &session_id,
            &private_beach_id,
            &join_code,
            &road_base_url,
            label,
            &viewer_peer_id,
            attempts,
            cancel.clone(),
        )
        .await
        {
            Ok(()) => {
                info!(
                    target = "private_beach",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    viewer_peer_id = %viewer_peer_id,
                    attempt = attempts,
                    "manager viewer disconnected cleanly"
                );
                state
                    .record_timeline_event(
                        &private_beach_id,
                        &session_id,
                        &viewer_peer_id,
                        "disconnect_clean",
                        json!({ "attempt": attempts }),
                    )
                    .await;
                last_error = None;
            }
            Err(err) => {
                let err_str = err.to_string();
                warn!(
                    target = "private_beach",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    viewer_peer_id = %viewer_peer_id,
                    attempt = attempts,
                    error = %err,
                    "manager viewer connection failed"
                );
                state
                    .record_timeline_event(
                        &private_beach_id,
                        &session_id,
                        &viewer_peer_id,
                        "connect_failed",
                        json!({
                            "attempt": attempts,
                            "error": err_str,
                        }),
                    )
                    .await;
                last_error = Some(err_str);
            }
        }
        metrics::MANAGER_VIEWER_CONNECTED
            .with_label_values(&[label_private.as_str(), label_session.as_str()])
            .set(0);
        let backoff = sleep(StdDuration::from_secs(3));
        tokio::pin!(backoff);
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            _ = &mut backoff => {}
        }
    }
}

async fn viewer_connect_once(
    state: &AppState,
    session_id: &str,
    private_beach_id: &str,
    join_code: &str,
    road_base_url: &str,
    label: &str,
    viewer_peer_id: &str,
    attempt: usize,
    cancel: CancellationToken,
) -> Result<(), ViewerError> {
    let gauge =
        metrics::MANAGER_VIEWER_CONNECTED.with_label_values(&[private_beach_id, session_id]);
    gauge.set(0);
    let gauge_guard = ViewerGaugeGuard::new(gauge.clone());
    let latency_hist =
        metrics::MANAGER_VIEWER_LATENCY_MS.with_label_values(&[private_beach_id, session_id]);

    state
        .record_timeline_event(
            private_beach_id,
            session_id,
            viewer_peer_id,
            "connect_start",
            json!({
                "attempt": attempt,
                "label": label,
            }),
        )
        .await;

    if cancel.is_cancelled() {
        return Ok(());
    }

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
    let mut attach_metadata = HashMap::new();
    attach_metadata.insert("role".into(), "viewer".into());
    attach_metadata.insert("peer_id".into(), viewer_peer_id.to_string());
    attach_metadata.insert("attempt".into(), attempt.to_string());
    let negotiated = negotiate_transport(
        &handle,
        Some(join_code),
        Some(label),
        false,
        Some(attach_metadata),
    )
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
    state
        .record_timeline_event(
            private_beach_id,
            session_id,
            viewer_peer_id,
            "connect_ok",
            json!({
                "attempt": attempt,
                "transport": "webrtc",
            }),
        )
        .await;
    gauge_guard.mark_connected();

    if cancel.is_cancelled() {
        return Ok(());
    }

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
        if cancel.is_cancelled() {
            debug!(
                target = "private_beach",
                session_id = %session_id,
                "viewer worker cancellation requested; closing transport"
            );
            return Ok(());
        }
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
                            let report_health = matches!(frame, WireHostFrame::Heartbeat { .. })
                                && viewer_state.claim_health_report_slot(Instant::now());
                            let frame_kind = match &frame {
                                WireHostFrame::Hello { subscription, .. } => {
                                    format!("hello:subscription={subscription}")
                                }
                                WireHostFrame::Grid {
                                    cols,
                                    history_rows,
                                    base_row,
                                    viewport_rows,
                                } => format!(
                                    "grid:cols={cols},history_rows={history_rows},base_row={base_row:?},viewport_rows={viewport_rows:?}"
                                ),
                                WireHostFrame::Snapshot { updates, .. } => {
                                    format!("snapshot:updates={}", updates.len())
                                }
                                WireHostFrame::SnapshotComplete { .. } => {
                                    "snapshot_complete".to_string()
                                }
                                WireHostFrame::Delta { updates, .. } => {
                                    format!("delta:updates={}", updates.len())
                                }
                                WireHostFrame::HistoryBackfill { updates, .. } => {
                                    format!("history_backfill:updates={}", updates.len())
                                }
                                WireHostFrame::InputAck { .. } => "input_ack".to_string(),
                                WireHostFrame::Cursor { .. } => "cursor".to_string(),
                                WireHostFrame::Heartbeat { .. } => "heartbeat".to_string(),
                                WireHostFrame::Extension { .. } => "extension".to_string(),
                                WireHostFrame::Shutdown => "shutdown".to_string(),
                            };
                            debug!(
                                target = "private_beach",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                frame = frame_kind,
                                bytes = bytes.len(),
                                "manager viewer received host frame"
                            );
                            trace!(
                                target = "private_beach.frames",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                raw_len = bytes.len(),
                                frame = ?frame,
                                "manager viewer decoded host frame content"
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
                            match viewer_state.handle_host_frame(&frame) {
                                Some(diff) => {
                                    let sequence = diff.sequence;
                                    trace!(
                                        target = "private_beach",
                                        session_id = %session_id,
                                        private_beach_id = %private_beach_id,
                                        seq = sequence,
                                        status = "diff_produced",
                                        "manager viewer produced state diff"
                                    );
                                    if let Err(err) =
                                        state.record_state(session_id, diff, false).await
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
                                None => {
                                    trace!(
                                        target = "private_beach",
                                        session_id = %session_id,
                                        private_beach_id = %private_beach_id,
                                        status = "consumed_no_diff",
                                        "manager viewer consumed frame without diff"
                                    );
                                }
                            }
                            if report_health {
                                if let Err(err) =
                                    persist_viewer_heartbeat(state, private_beach_id, session_id)
                                        .await
                                {
                                    debug!(
                                        target = "private_beach",
                                        session_id = %session_id,
                                        private_beach_id = %private_beach_id,
                                        error = %err,
                                        "manager viewer failed to record heartbeat"
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
                            let truncated = if bytes.len() > 64 {
                                format!("{} bytes (first 64: {:02x?} …)", bytes.len(), &bytes[..64])
                            } else {
                                format!("{} bytes: {:02x?}", bytes.len(), bytes.as_slice())
                            };
                            warn!(
                                target = "private_beach",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                payload = %truncated,
                                status = "decode_failed",
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

async fn persist_viewer_heartbeat(
    state: &AppState,
    private_beach_id: &str,
    session_id: &str,
) -> Result<(), StateError> {
    let queue_depth = state
        .pending_actions_count(private_beach_id, session_id)
        .await?;
    if should_log_custom_event("viewer_heartbeat", session_id, StdDuration::from_secs(30)) {
        info!(
            target = "private_beach.health",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            queue_depth,
            "viewer heartbeat recorded"
        );
    }
    let heartbeat = HealthHeartbeat {
        queue_depth,
        cpu_load: None,
        memory_bytes: None,
        degraded: false,
        warnings: Vec::new(),
    };
    state.record_health(session_id, heartbeat).await
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

#[derive(Debug, thiserror::Error)]
enum ControllerForwarderError {
    #[error("session join failed: {0}")]
    Join(String),
    #[error("transport negotiation failed: {0}")]
    Negotiation(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error(transparent)]
    State(#[from] StateError),
    #[error("unsupported transport role")]
    UnsupportedTransport,
}

async fn run_controller_forwarder(
    state: AppState,
    session_id: String,
    private_beach_id: String,
    join_code: String,
    cancel: CancellationToken,
) {
    let mut attempts = 0usize;
    loop {
        if cancel.is_cancelled() {
            info!(
                target = "controller.forwarder",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                "controller forwarder cancelled"
            );
            break;
        }
        attempts = attempts.saturating_add(1);
        info!(
            target = "controller.forwarder",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            attempts,
            "controller forwarder attempt starting"
        );
        match controller_forwarder_once(
            &state,
            &session_id,
            &private_beach_id,
            &join_code,
            cancel.clone(),
        )
        .await
        {
            Ok(()) => {
                info!(
                    target = "controller.forwarder",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    "controller forwarder stopped"
                );
                break;
            }
            Err(err) => {
                warn!(
                    target = "controller.forwarder",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    error = %err,
                    attempts,
                    "controller forwarder failed; retrying"
                );
                let backoff = sleep(StdDuration::from_secs(3));
                tokio::pin!(backoff);
                tokio::select! {
                    _ = cancel.cancelled() => {
                        break;
                    }
                    _ = &mut backoff => {}
                }
            }
        }
    }
}

async fn controller_forwarder_once(
    state: &AppState,
    session_id: &str,
    private_beach_id: &str,
    join_code: &str,
    cancel: CancellationToken,
) -> Result<(), ControllerForwarderError> {
    let labels = [CONTROLLER_CHANNEL_LABEL, LEGACY_CONTROLLER_CHANNEL_LABEL];
    let mut last_err: Option<ControllerForwarderError> = None;
    for label in labels {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match controller_forwarder_once_with_label(
            state,
            session_id,
            private_beach_id,
            join_code,
            label,
            cancel.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                continue;
            }
        }
    }
    Err(last_err.unwrap_or(ControllerForwarderError::Join(
        "controller forwarder negotiation failed".into(),
    )))
}

async fn controller_forwarder_once_with_label(
    state: &AppState,
    session_id: &str,
    private_beach_id: &str,
    join_code: &str,
    label: &str,
    cancel: CancellationToken,
) -> Result<(), ControllerForwarderError> {
    let attempt_started = Instant::now();
    let config = SessionConfig::new(&state.road_base_url)
        .map_err(|err| ControllerForwarderError::Join(err.to_string()))?;
    let manager = SessionManager::new(config)
        .map_err(|err| ControllerForwarderError::Join(err.to_string()))?;
    let fallback_base = manager.config().base_url().clone();
    info!(
        target = "controller.forwarder",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        label = %label,
        "controller forwarder preparing join"
    );
    let joined = manager
        .join(session_id, Some(join_code), None, Some(label), false)
        .await
        .map_err(|err| {
            warn!(
                target = "controller.forwarder",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                label = %label,
                wait_ms = attempt_started.elapsed().as_millis() as u64,
                error = %err,
                "controller forwarder join failed"
            );
            ControllerForwarderError::Join(err.to_string())
        })?;
    let mut handle = joined.into_handle();
    let join_elapsed_ms = attempt_started.elapsed().as_millis() as u64;
    trace!(
        target = "controller.forwarder",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        label = %label,
        wait_ms = join_elapsed_ms,
        "controller forwarder joined session"
    );
    rewrite_loopback_transports(&mut handle, &fallback_base)
        .map_err(|err| ControllerForwarderError::Join(err.to_string()))?;
    // Pre-derive session keys so the WebRTC handshake does not spend time in Argon2 later.
    warm_session_key(Some(join_code), session_id).await;
    let negotiate_started = Instant::now();
    info!(
        target = "controller.forwarder",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        requested_label = %label,
        "controller forwarder negotiating transport"
    );
    let mut forwarder_metadata = HashMap::new();
    forwarder_metadata.insert("role".into(), "manager".into());
    forwarder_metadata.insert("session_id".into(), session_id.to_string());
    forwarder_metadata.insert("peer_id".into(), Uuid::new_v4().to_string());
    if let Some(token) = manager_bearer_token() {
        forwarder_metadata.insert("bearer".into(), token);
    } else {
        warn!(
            target = "controller.forwarder",
            session_id = %session_id,
            "manager bearer token missing; WebRTC peer auth may fail"
        );
    }
    let negotiated = negotiate_transport(
        &handle,
        Some(join_code),
        Some(label),
        false,
        Some(forwarder_metadata),
    )
    .await
    .map_err(|err| {
        warn!(
            target = "controller.forwarder",
            session_id = %session_id,
            private_beach_id = %private_beach_id,
            requested_label = %label,
            wait_ms = negotiate_started.elapsed().as_millis() as u64,
            error = %err,
            "controller forwarder negotiation failed"
        );
        ControllerForwarderError::Negotiation(err.to_string())
    })?;
    match negotiated {
        NegotiatedTransport::Single(NegotiatedSingle {
            transport,
            webrtc_channels,
            metadata,
            ..
        }) => {
            let peer_session_id = metadata.get("peer_session_id").cloned();
            info!(
                target = "controller.forwarder",
                session_id = %session_id,
                private_beach_id = %private_beach_id,
                requested_label = %label,
                negotiated_label = metadata
                    .get("label")
                    .map(|v| v.to_string()),
                peer_session_id = peer_session_id.as_deref(),
                transport = ?transport.kind(),
                has_webrtc_channels = webrtc_channels.is_some(),
                join_ms = join_elapsed_ms,
                negotiate_ms = negotiate_started.elapsed().as_millis() as u64,
                "controller forwarder negotiated transport"
            );
            let connection_label = metadata.get("label").cloned().or_else(|| {
                debug!(
                    target = "controller.forwarder",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    requested_label = %label,
                    "controller transport metadata missing label; using requested value"
                );
                Some(label.to_string())
            });
            match drive_controller_forwarder(
                state,
                private_beach_id,
                session_id,
                transport,
                webrtc_channels,
                connection_label.clone(),
                peer_session_id.clone(),
                cancel,
            )
            .await
            {
                Ok(()) => {
                    debug!(
                        target = "controller.forwarder",
                        session_id = %session_id,
                        private_beach_id = %private_beach_id,
                        label = connection_label.as_deref(),
                        "controller forwarder completed"
                    );
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        NegotiatedTransport::WebRtcOfferer { .. } => {
            Err(ControllerForwarderError::UnsupportedTransport)
        }
    }
}

struct PendingControllerAction {
    action: ActionCommand,
    sent_at: Instant,
}

fn remove_pending_by_id(
    pending: &mut HashMap<u64, PendingControllerAction>,
    action_id: &str,
) -> Option<u64> {
    let seq = pending.iter().find_map(|(seq, pending)| {
        if pending.action.id == action_id {
            Some(*seq)
        } else {
            None
        }
    })?;
    pending.remove(&seq);
    Some(seq)
}

enum ForwarderEvent {
    Ack(u64),
    AckById(ActionAck),
    State(StateDiff),
    Health(HealthHeartbeat),
    Shutdown(String),
}

struct ForwarderReader {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ForwarderReader {
    fn spawn(transport: Arc<dyn Transport>, tx: mpsc::UnboundedSender<ForwarderEvent>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let handle = thread::spawn(move || {
            let timeout = StdDuration::from_millis(250);
            while !stop_clone.load(Ordering::Relaxed) {
                match transport.recv(timeout) {
                    Ok(message) => match message.payload {
                        Payload::Binary(bytes) => match decode_host_frame_binary(&bytes) {
                            Ok(frame) => match frame {
                                WireHostFrame::InputAck { seq } => {
                                    let _ = tx.send(ForwarderEvent::Ack(seq));
                                }
                                WireHostFrame::Extension { frame } => {
                                    extensions::publish(transport.id(), frame);
                                }
                                WireHostFrame::Shutdown => {
                                    let _ = tx.send(ForwarderEvent::Shutdown(
                                        "host requested shutdown".into(),
                                    ));
                                    break;
                                }
                                _ => {}
                            },
                            Err(err) => {
                                trace!(
                                    target = "controller.forwarder",
                                    transport_id = %transport.id().0,
                                    error = %err,
                                    "failed to decode host frame"
                                );
                            }
                        },
                        Payload::Text(text) => {
                            let trimmed = text.trim();
                            if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                                continue;
                            }
                        }
                    },
                    Err(TransportError::Timeout) => {}
                    Err(TransportError::ChannelClosed) => {
                        let _ = tx.send(ForwarderEvent::Shutdown(
                            "controller transport closed".into(),
                        ));
                        break;
                    }
                    Err(err) => {
                        trace!(
                            target = "controller.forwarder",
                            transport_id = %transport.id().0,
                            error = %err,
                            "controller transport recv error"
                        );
                        let _ = tx.send(ForwarderEvent::Shutdown(format!(
                            "controller transport error: {err}"
                        )));
                        break;
                    }
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for ForwarderReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

async fn drive_controller_forwarder(
    state: &AppState,
    private_beach_id: &str,
    session_id: &str,
    primary_transport: Arc<dyn Transport>,
    _webrtc_channels: Option<WebRtcChannels>,
    metadata_label: Option<String>,
    peer_session_id: Option<String>,
    cancel: CancellationToken,
) -> Result<(), ControllerForwarderError> {
    info!(
        target = "controller.forwarder",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        metadata_label = metadata_label.as_deref(),
        peer_session_id = peer_session_id.as_deref(),
        "controller forwarder starting"
    );
    let (mut transport, mut transport_label, mut via_fast_path) = select_controller_transport(
        primary_transport.clone(),
        None,
        metadata_label.as_deref(),
        false,
        session_id,
        private_beach_id,
    )
    .await;
    let t_id = transport.id();
    let t_peer = transport.peer();
    info!(
        target = "controller.forwarder",
        session_id = %session_id,
        private_beach_id = %private_beach_id,
        transport = transport_label,
        via_fast_path,
        transport_id = %t_id.0,
        peer_id = %t_peer.0,
        "controller forwarder connected"
    );
    state
        .emit_host_devtools_event(
            session_id,
            "host.transport:connected",
            DevtoolsEventLevel::Info,
            Some(serde_json::json!({
                "privateBeachId": private_beach_id,
                "transport": transport_label,
                "viaFastPath": via_fast_path,
                "transportId": t_id.0,
                "peerId": t_peer.0,
                "peerSessionId": peer_session_id,
            })),
        )
        .await;

    let initial_status = if via_fast_path {
        PairingTransportStatus::webrtc(now_ms())
    } else {
        PairingTransportStatus::http_fallback(now_ms(), None)
    };
    state
        .update_pairing_transport_status(session_id, initial_status)
        .await;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut reader_guard = ForwarderReader::spawn(transport.clone(), event_tx.clone());
    let mut framed_rx = framed::subscribe(transport.id(), "controller");
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        loop {
            match framed_rx.recv().await {
                Ok(frame) => match frame.kind.as_str() {
                    "ack" => {
                        if let Ok(ack) = serde_json::from_slice::<ActionAck>(&frame.payload) {
                            let _ = event_tx_clone.send(ForwarderEvent::AckById(ack));
                        }
                    }
                    "state" => {
                        if let Ok(diff) = serde_json::from_slice::<StateDiff>(&frame.payload) {
                            let _ = event_tx_clone.send(ForwarderEvent::State(diff));
                        }
                    }
                    "health" => {
                        if let Ok(heartbeat) =
                            serde_json::from_slice::<HealthHeartbeat>(&frame.payload)
                        {
                            let _ = event_tx_clone.send(ForwarderEvent::Health(heartbeat));
                        }
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        target = "controller.forwarder",
                        skipped, "controller framed subscriber lagged"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let mut pending: HashMap<u64, PendingControllerAction> = HashMap::new();
    let mut next_seq: u64 = 0;
    let mut idle_delay = tokio::time::interval(StdDuration::from_millis(200));
    idle_delay.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_ack_at: Option<Instant> = None;
    let mut last_ack_transport: Option<String> = None;
    let mut last_send_at: Option<Instant> = None;

    // If fast-path appears healthy (sends succeed) but no acknowledgements are
    // observed for an extended period, treat the transport as stalled and
    // immediately fall back to the primary transport (HTTP). This prevents
    // unbounded queue growth when the selected data channel is misbound or the
    // remote is not consuming inputs.
    const ACK_STALL_DEADLINE: StdDuration = StdDuration::from_millis(1500);

    let labels = [private_beach_id, session_id];

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let inflight = pending.len();
                let drained = std::mem::take(&mut pending);
                fail_pending_actions(
                    state,
                    session_id,
                    drained,
                    via_fast_path,
                    "controller forwarder cancelled",
                )
                .await;
                let queue_depth = match state
                    .pending_actions_count(private_beach_id, session_id)
                    .await
                {
                    Ok(value) => value,
                    Err(err) => {
                        trace!(
                            target = "controller.forwarder",
                            session_id = %session_id,
                            private_beach_id = %private_beach_id,
                            error = %err,
                            "failed to read queue depth after cancellation"
                        );
                        0
                    }
                };
                info!(
                    target = "controller.forwarder",
                    session_id = %session_id,
                    private_beach_id = %private_beach_id,
                    inflight,
                    queue_depth,
                    transport = transport_label,
                    "controller forwarder shutting down due to cancellation"
                );
                state
                    .emit_host_devtools_event(
                        session_id,
                        "host.transport:stopped",
                        DevtoolsEventLevel::Info,
                        Some(serde_json::json!({
                            "reason": "cancelled",
                            "inflight": inflight,
                            "queueDepth": queue_depth,
                            "transport": transport_label,
                        })),
                    )
                    .await;
                return Ok(());
            }
            event = event_rx.recv() => {
                match event {
                    Some(ForwarderEvent::Ack(seq)) => {
                        let inflight_before = pending.len();
                        if let Some(pending_action) = pending.remove(&seq) {
                            let latency = pending_action.sent_at.elapsed().as_millis() as u64;
                            let inflight_after = pending.len();
                            let ack = ActionAck {
                                id: pending_action.action.id.clone(),
                                status: AckStatus::Ok,
                                applied_at: SystemTime::now(),
                                latency_ms: Some(latency),
                                error_code: None,
                                error_message: None,
                            };
                            last_ack_at = Some(Instant::now());
                            last_ack_transport = Some(transport_label.to_string());
                            state
                                .ack_actions(session_id, vec![ack], None, via_fast_path)
                                .await?;
                            metrics::ACTIONS_ACKED.with_label_values(&labels).inc();
                            metrics::ACTION_LATENCY_MS
                                .with_label_values(&labels)
                                .observe(latency as f64);
                            debug!(
                                target = "controller.delivery",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                action_id = %pending_action.action.id,
                                seq,
                                transport = transport_label,
                                transport_id = %transport.id().0,
                                inflight_before,
                                inflight_after,
                                "controller action acked"
                            );
                        } else {
                            trace!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                seq,
                                "received ack for unknown sequence"
                            );
                        }
                    }
                    Some(ForwarderEvent::AckById(ack)) => {
                        let inflight_before = pending.len();
                        let removed_seq = remove_pending_by_id(&mut pending, &ack.id);
                        if removed_seq.is_some() {
                            metrics::ACTIONS_ACKED.with_label_values(&labels).inc();
                            last_ack_at = Some(Instant::now());
                            last_ack_transport = Some(transport_label.to_string());
                            debug!(
                                target = "controller.delivery",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                action_id = %ack.id,
                                seq = removed_seq,
                                transport = transport_label,
                                transport_id = %transport.id().0,
                                inflight_before,
                                inflight_after = pending.len(),
                                "controller action acked by id"
                            );
                        } else {
                            trace!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                action_id = %ack.id,
                                "received action ack for unknown id"
                            );
                        }
                        if let Err(err) =
                            state.ack_actions(session_id, vec![ack.clone()], None, via_fast_path).await
                        {
                            warn!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                "failed to remove action after extension ack"
                            );
                        }
                    }
                    Some(ForwarderEvent::State(diff)) => {
                        if let Err(err) = state.record_state(session_id, diff, via_fast_path).await {
                            warn!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                "failed to record state from extension frame"
                            );
                        }
                    }
                    Some(ForwarderEvent::Health(heartbeat)) => {
                        if let Err(err) = state.record_health(session_id, heartbeat).await {
                            warn!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                error = %err,
                                "failed to record health from extension frame"
                            );
                        }
                    }
                    Some(ForwarderEvent::Shutdown(reason)) => {
                        let inflight = pending.len();
                        let queue_depth = match state
                            .pending_actions_count(private_beach_id, session_id)
                            .await
                        {
                            Ok(value) => value,
                            Err(err) => {
                                trace!(
                                    target = "controller.forwarder",
                                    session_id = %session_id,
                                    private_beach_id = %private_beach_id,
                                    error = %err,
                                    "failed to read queue depth after controller shutdown"
                                );
                                0
                            }
                        };
                        let drained = std::mem::take(&mut pending);
                        fail_pending_actions(state, session_id, drained, via_fast_path, &reason).await;
                        let reason_detail = reason.clone();
                        warn!(
                            target = "controller.forwarder",
                            session_id = %session_id,
                            private_beach_id = %private_beach_id,
                            inflight,
                            queue_depth,
                            transport = transport_label,
                            reason = %reason,
                            "controller transport closed; exiting"
                        );
                        state
                            .emit_host_devtools_event(
                                session_id,
                                "host.transport:closed",
                                DevtoolsEventLevel::Error,
                                Some(serde_json::json!({
                                    "reason": reason_detail,
                                    "queueDepth": queue_depth,
                                    "inflight": inflight,
                                    "transport": transport_label,
                                })),
                            )
                            .await;
                        return Err(ControllerForwarderError::Transport(reason));
                    }
                    None => {
                        let inflight = pending.len();
                        let queue_depth = match state
                            .pending_actions_count(private_beach_id, session_id)
                            .await
                        {
                            Ok(value) => value,
                            Err(err) => {
                                trace!(
                                    target = "controller.forwarder",
                                    session_id = %session_id,
                                    private_beach_id = %private_beach_id,
                                    error = %err,
                                    "failed to read queue depth after ack listener closed"
                                );
                                0
                            }
                        };
                        let drained = std::mem::take(&mut pending);
                        fail_pending_actions(
                            state,
                            session_id,
                            drained,
                            via_fast_path,
                            "ack listener closed",
                        )
                        .await;
                        warn!(
                            target = "controller.forwarder",
                            session_id = %session_id,
                            private_beach_id = %private_beach_id,
                            inflight,
                            queue_depth,
                            transport = transport_label,
                            "controller forwarder ack listener closed unexpectedly"
                        );
                        state
                            .emit_host_devtools_event(
                                session_id,
                                "host.transport:closed",
                                DevtoolsEventLevel::Error,
                                Some(serde_json::json!({
                                    "reason": "ack_listener_closed",
                                    "queueDepth": queue_depth,
                                    "inflight": inflight,
                                    "transport": transport_label,
                                })),
                            )
                            .await;
                        return Err(ControllerForwarderError::Transport(
                            "ack listener closed".into(),
                        ));
                    }
                }
            }
            _ = idle_delay.tick() => {
                // Detect ack stalls: if any action has been inflight longer than
                // the deadline without an acknowledgement, fail the pending set
                // and switch away from the current transport.
                if !pending.is_empty() {
                    if let Some(oldest) = pending.values().map(|p| p.sent_at).min() {
                        if oldest.elapsed() > ACK_STALL_DEADLINE {
                            let inflight = pending.len();
                            let drained = std::mem::take(&mut pending);
                            let reason = format!(
                                "no action acknowledgements for >{:?}; treating as stalled",
                                ACK_STALL_DEADLINE
                            );
                            let now = Instant::now();
                            let diag = transport_diagnostics(transport.id());
                            let stall_snapshot = serde_json::json!({
                                "inflightByTransport": { transport_label: inflight },
                                "lastAckTransport": last_ack_transport.clone(),
                                "lastAckAgeMs": last_ack_at.as_ref().map(|ts| now.saturating_duration_since(*ts).as_millis()),
                                "lastSendAgeMs": last_send_at.as_ref().map(|ts| now.saturating_duration_since(*ts).as_millis()),
                                "bufferedAmount": diag.as_ref().and_then(|d| d.last_buffered_amount),
                                "bufferedHighWater": diag.as_ref().map(|d| d.buffered_high_water),
                                "channel": diag.as_ref().and_then(|d| d.channel_label.clone()),
                                "pcState": diag.as_ref().and_then(|d| d.last_pc_state.clone()),
                                "iceState": diag.as_ref().and_then(|d| d.last_ice_state.clone()),
                                "transportId": transport.id().0,
                            });
                            fail_pending_actions(state, session_id, drained, via_fast_path, &reason).await;
                            warn!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                inflight,
                                stall_snapshot = ?stall_snapshot,
                                transport = transport_label,
                                "ack stall detected on unified transport; stopping forwarder"
                            );
                            state
                                .emit_host_devtools_event(
                                    session_id,
                                    "host.transport:ack-stall",
                                    DevtoolsEventLevel::Error,
                                    Some(serde_json::json!({
                                        "inflight": inflight,
                                        "transport": transport_label,
                                        "deadlineMs": ACK_STALL_DEADLINE.as_millis(),
                                        "stallSnapshot": stall_snapshot,
                                    })),
                                )
                                .await;
                            return Err(ControllerForwarderError::Transport(reason));
                        }
                    }
                }
                let actions = state.poll_actions(session_id).await?;
                if actions.is_empty() {
                    if !pending.is_empty()
                        && should_log_custom_event(
                            "controller_forwarder_pending",
                            session_id,
                            StdDuration::from_secs(20),
                        )
                    {
                        warn!(
                            target = "controller.forwarder",
                            session_id = %session_id,
                            private_beach_id = %private_beach_id,
                            inflight = pending.len(),
                            transport = transport_label,
                            "controller forwarder awaiting acknowledgements before requesting more actions"
                        );
                    }
                    continue;
                }
                if should_log_custom_event(
                    "controller_forwarder_dispatch",
                    session_id,
                    StdDuration::from_secs(10),
                ) {
                    info!(
                        target = "controller.forwarder",
                        session_id = %session_id,
                        private_beach_id = %private_beach_id,
                        fetched = actions.len(),
                        inflight = pending.len(),
                        transport = transport_label,
                        via_fast_path,
                        "controller forwarder dispatching actions to host"
                    );
                }
                for action in actions {
                    let action_id = action.id.clone();
                    match action_terminal_bytes(&action) {
                        Ok(bytes) => {
                            let inflight_before = pending.len();
                            next_seq = next_seq.saturating_add(1);
                            let controller_payload = match serde_json::to_vec(&serde_json::json!({
                                "action_id": action_id.clone(),
                                "bytes": bytes,
                            })) {
                                Ok(payload) => payload,
                                Err(err) => {
                                    warn!(
                                        target = "controller.forwarder",
                                        session_id = %session_id,
                                        error = %err,
                                        "failed to encode controller input payload"
                                    );
                                    continue;
                                }
                            };
                            let sent_at = loop {
                                let send_result = match transport
                                    .send_namespaced("controller", "input", &controller_payload)
                                {
                                    Ok(seq) => Ok(seq),
                                    Err(TransportError::Setup(_)) => {
                                        let encoded =
                                            encode_client_frame_binary(&ClientFrame::Input {
                                                seq: next_seq,
                                                data: controller_payload.clone(),
                                            });
                                        transport.send_bytes(&encoded)
                                    }
                                    Err(err) => Err(err),
                                };
                                match send_result {
                                    Ok(_) => {
                                        metrics::ACTIONS_DELIVERED.with_label_values(&labels).inc();
                                        trace!(
                                            target = "controller.forwarder",
                                            session_id = %session_id,
                                            private_beach_id = %private_beach_id,
                                            via_fast_path,
                                            transport = transport_label,
                                            action_id = %action_id,
                                            payload_len = controller_payload.len(),
                                            "sent controller action"
                                        );
                                        break Instant::now();
                                    }
                                    Err(err)
                                        if via_fast_path
                                            && matches!(err, TransportError::ChannelClosed | TransportError::Timeout) =>
                                    {
                                        let inflight = pending.len();
                                        let diag = transport_diagnostics(transport.id());
                                        let fallback_snapshot = serde_json::json!({
                                            "inflightByTransport": { transport_label: inflight },
                                            "lastAckTransport": last_ack_transport.clone(),
                                            "lastAckAgeMs": last_ack_at.as_ref().map(|ts| ts.elapsed().as_millis()),
                                            "lastSendAgeMs": last_send_at.as_ref().map(|ts| ts.elapsed().as_millis()),
                                            "bufferedAmount": diag.as_ref().and_then(|d| d.last_buffered_amount),
                                            "bufferedHighWater": diag.as_ref().map(|d| d.buffered_high_water),
                                            "channel": diag.as_ref().and_then(|d| d.channel_label.clone()),
                                            "pcState": diag.as_ref().and_then(|d| d.last_pc_state.clone()),
                                            "iceState": diag.as_ref().and_then(|d| d.last_ice_state.clone()),
                                            "transportId": transport.id().0,
                                        });
                                        let drained = std::mem::take(&mut pending);
                                        let reason = format!("fast-path send failed: {err}");
                                        fail_pending_actions(state, session_id, drained, true, &reason).await;
                                        warn!(
                                            target = "controller.forwarder",
                                            session_id = %session_id,
                                            private_beach_id = %private_beach_id,
                                            inflight,
                                            error = %err,
                                            fallback_snapshot = ?fallback_snapshot,
                                            "unified send failed; terminating controller forwarder"
                                        );
                                        state
                                            .emit_host_devtools_event(
                                                session_id,
                                                "host.transport:error",
                                                DevtoolsEventLevel::Error,
                                                Some(serde_json::json!({
                                                    "reason": format!("unified_send_error:{err}"),
                                                    "inflight": inflight,
                                                    "snapshot": fallback_snapshot,
                                                })),
                                            )
                                            .await;
                                        return Err(ControllerForwarderError::Transport(reason));
                                    }
                                    Err(err) => {
                                        let drained = std::mem::take(&mut pending);
                                        let reason = err.to_string();
                                        fail_pending_actions(state, session_id, drained, via_fast_path, &reason).await;
                                        state
                                            .emit_host_devtools_event(
                                                session_id,
                                                "host.transport:error",
                                                DevtoolsEventLevel::Error,
                                                Some(serde_json::json!({
                                                    "reason": reason,
                                                    "transport": transport_label,
                                                })),
                                            )
                                            .await;
                                        return Err(ControllerForwarderError::Transport(reason));
                                    }
                                }
                            };
                            last_send_at = Some(sent_at);
                            debug!(
                                target = "controller.delivery",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                action_id = %action.id,
                                seq = next_seq,
                                transport = transport_label,
                                "forwarded controller action"
                            );
                            let action_id = action.id.clone();
                            pending.insert(
                                next_seq,
                                PendingControllerAction {
                                    action,
                                    sent_at,
                                },
                            );
                            let inflight_after = pending.len();
                            let diag = transport_diagnostics(transport.id());
                            info!(
                                target = "controller.delivery",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                action_id = %action_id,
                                seq = next_seq,
                                transport = transport_label,
                                transport_id = %transport.id().0,
                                inflight_before,
                                inflight_after,
                                buffered_amount = diag.as_ref().and_then(|d| d.last_buffered_amount),
                                buffered_high_water = diag.as_ref().map(|d| d.buffered_high_water),
                                "controller action sent"
                            );
                            state
                                .emit_connector_events_for_child(
                                    session_id,
                                    "connector.action:forwarded",
                                    DevtoolsEventLevel::Info,
                                    Some(serde_json::json!({
                                        "actionId": action_id,
                                        "transport": transport_label,
                                        "viaFastPath": via_fast_path,
                                        "sequence": next_seq,
                                    })),
                                )
                                .await;
                        }
                        Err(err_msg) => {
                            warn!(
                                target = "controller.forwarder",
                                session_id = %session_id,
                                private_beach_id = %private_beach_id,
                                error = %err_msg,
                                "rejecting unsupported action"
                            );
                            state
                                .emit_connector_events_for_child(
                                    session_id,
                                    "connector.action:rejected",
                                    DevtoolsEventLevel::Warn,
                                    Some(serde_json::json!({
                                        "reason": err_msg,
                                        "actionId": action.id,
                                    })),
                                )
                                .await;
                            let ack = ActionAck {
                                id: action.id.clone(),
                                status: AckStatus::Rejected,
                                applied_at: SystemTime::now(),
                                latency_ms: None,
                                error_code: Some("unsupported_action".into()),
                                error_message: Some(err_msg),
                            };
                            if let Err(err) = state
                                .ack_actions(session_id, vec![ack], None, via_fast_path)
                                .await
                            {
                                warn!(
                                    target = "controller.forwarder",
                                    session_id = %session_id,
                                    error = %err,
                                    "failed to reject unsupported action"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn select_controller_transport(
    primary_transport: Arc<dyn Transport>,
    _webrtc_channels: Option<WebRtcChannels>,
    _metadata_label: Option<&str>,
    _fast_path_enabled: bool,
    _session_id: &str,
    _private_beach_id: &str,
) -> (Arc<dyn Transport>, &'static str, bool) {
    let kind = primary_transport.kind();
    let via_fast_path = matches!(kind, TransportKind::WebRtc | TransportKind::WebSocket);
    let label = match kind {
        TransportKind::WebRtc => "webrtc",
        TransportKind::WebSocket => "websocket",
        TransportKind::Ipc => "ipc",
    };
    (primary_transport, label, via_fast_path)
}

fn action_terminal_bytes(action: &ActionCommand) -> Result<Vec<u8>, String> {
    if action.action_type.as_str() != "terminal_write" {
        return Err(format!("unsupported action type {}", action.action_type));
    }
    let bytes = action
        .payload
        .get("bytes")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal_write payload missing bytes".to_string())?;
    Ok(bytes.as_bytes().to_vec())
}

async fn fail_pending_actions(
    state: &AppState,
    session_id: &str,
    pending: HashMap<u64, PendingControllerAction>,
    via_fast_path: bool,
    reason: &str,
) {
    if pending.is_empty() {
        return;
    }
    let now = SystemTime::now();
    let acks: Vec<ActionAck> = pending
        .into_values()
        .map(|pending_action| ActionAck {
            id: pending_action.action.id,
            status: AckStatus::Preempted,
            applied_at: now,
            latency_ms: None,
            error_code: Some("controller_forwarder".into()),
            error_message: Some(reason.to_string()),
        })
        .collect();
    if let Err(err) = state
        .ack_actions(session_id, acks, None, via_fast_path)
        .await
    {
        warn!(
            target = "controller.forwarder",
            session_id = %session_id,
            error = %err,
            "failed to ack pending actions after disconnect"
        );
    }
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

fn log_session_attachment(private_beach_id: &str, session_id: &str, method: &str, status: &str) {
    info!(
        target = "private_beach.sessions",
        private_beach_id = %private_beach_id,
        session_id = %session_id,
        method,
        status,
        "session attachment updated"
    );
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ice_servers: Option<Vec<TurnIceServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ice_servers_expires_at_ms: Option<i64>,
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
    use serde_json::Value;
    use std::{
        future::Future,
        mem,
        pin::Pin,
        sync::{Arc, RwLock},
    };

    type ViewerOverride = Arc<
        dyn Fn(
                AppState,
                String,
                String,
                String,
                String,
                String,
            ) -> Pin<Box<dyn Future<Output = ()> + Send>>
            + Send
            + Sync,
    >;

    static VIEWER_WORKER_OVERRIDE: Lazy<RwLock<Option<ViewerOverride>>> =
        Lazy::new(|| RwLock::new(None));

    pub fn set_viewer_worker_override<F, Fut>(f: Option<F>)
    where
        F: Fn(AppState, String, String, String, String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut guard = VIEWER_WORKER_OVERRIDE.write().unwrap();
        *guard = f.map(|func| {
            Arc::new(
                move |state, session_id, private_beach_id, passcode, base_url, viewer_peer_id| {
                    Box::pin(func(
                        state,
                        session_id,
                        private_beach_id,
                        passcode,
                        base_url,
                        viewer_peer_id,
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

    static MANAGER_HANDSHAKES: Lazy<RwLock<Vec<(String, Value)>>> =
        Lazy::new(|| RwLock::new(Vec::new()));

    pub fn record_manager_handshake(session_id: &str, payload: &Value) {
        let mut guard = MANAGER_HANDSHAKES.write().unwrap();
        guard.push((session_id.to_string(), payload.clone()));
    }

    pub fn take_manager_handshakes() -> Vec<(String, Value)> {
        let mut guard = MANAGER_HANDSHAKES.write().unwrap();
        let mut out = Vec::new();
        mem::swap(&mut *guard, &mut out);
        out
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

    static RTC_READY_OVERRIDES: Lazy<RwLock<HashMap<String, bool>>> =
        Lazy::new(|| RwLock::new(HashMap::new()));

    pub fn set_rtc_ready_override(session_id: &str, ready: bool) {
        RTC_READY_OVERRIDES
            .write()
            .unwrap()
            .insert(session_id.to_string(), ready);
    }

    pub fn rtc_ready_override(session_id: &str) -> Option<bool> {
        RTC_READY_OVERRIDES.read().unwrap().get(session_id).copied()
    }

    pub fn clear_rtc_ready_override() {
        RTC_READY_OVERRIDES.write().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support;
    use beach_buggy::{ActionCommand, HarnessType, RegisterSessionRequest};
    use beach_client_core::cache::terminal::packed::{pack_color_default, pack_color_rgb, StyleId};
    use beach_client_core::protocol::{
        HostFrame as WireHostFrame, Lane, LaneBudgetFrame, SyncConfigFrame, Update as WireUpdate,
    };
    use beach_client_core::{TransportId, TransportKind, TransportMessage};
    use chrono::Utc;
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration as StdDuration, SystemTime};
    use tokio::sync::Notify;
    use tokio::time::{sleep, timeout, Duration};

    #[test_timeout::tokio_timeout_test(10)]
    #[serial_test::serial]
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
                  _base_url: String,
                  _viewer_peer_id: String| {
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
            transport_mode: Some(TransportMode::FastPath),
        };
        state.register_session(register).await.unwrap();

        let mut receiver = state.subscribe_session("sess-view").await;

        state.spawn_viewer_worker("sess-view").await.unwrap();

        timeout(Duration::from_secs(2), {
            let invocation_count = invocation_count.clone();
            async move {
                while invocation_count.load(Ordering::SeqCst) < 1 {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await
        .expect("viewer worker invocation");

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

    #[test_timeout::tokio_timeout_test(10)]
    #[serial_test::serial]
    async fn spawn_viewer_worker_is_idempotent_while_running() {
        test_support::clear_viewer_worker_override();
        let _guard = OverrideGuard;

        let invocation_count = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(Notify::new());
        test_support::set_viewer_worker_override(Some({
            let invocation_count = invocation_count.clone();
            let notify = notify.clone();
            move |state: AppState,
                  session_id: String,
                  private_beach_id: String,
                  passcode: String,
                  _base_url: String,
                  _viewer_peer_id: String| {
                let notify = notify.clone();
                let invocation_count = invocation_count.clone();
                async move {
                    assert_eq!(session_id, "sess-idempotent");
                    assert_eq!(private_beach_id, "pb-idempotent");
                    assert_eq!(passcode, "secret");
                    invocation_count.fetch_add(1, Ordering::SeqCst);
                    // Hold the worker open until the test releases it.
                    notify.notified().await;
                    // Touch state once to mirror normal behavior.
                    let diff = beach_buggy::StateDiff {
                        sequence: 1,
                        emitted_at: SystemTime::now(),
                        payload: json!({ "ping": "pong" }),
                    };
                    state.record_state(&session_id, diff, false).await.unwrap();
                }
            }
        }));

        let state = AppState::new();
        let register = RegisterSessionRequest {
            session_id: "sess-idempotent".into(),
            private_beach_id: "pb-idempotent".into(),
            harness_type: HarnessType::TerminalShim,
            capabilities: vec![],
            location_hint: None,
            metadata: None,
            version: "1.0.0".into(),
            viewer_passcode: Some("secret".into()),
            transport_mode: Some(TransportMode::FastPath),
        };
        state.register_session(register).await.unwrap();

        state.spawn_viewer_worker("sess-idempotent").await.unwrap();
        timeout(Duration::from_secs(2), {
            let invocation_count = invocation_count.clone();
            async move {
                while invocation_count.load(Ordering::SeqCst) < 1 {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        })
        .await
        .expect("first viewer worker invocation");

        // Second spawn while the first worker is still running should be a no-op.
        state.spawn_viewer_worker("sess-idempotent").await.unwrap();
        sleep(Duration::from_millis(200)).await;
        assert_eq!(
            invocation_count.load(Ordering::SeqCst),
            1,
            "viewer worker should not respawn while active"
        );

        // Let the worker finish cleanly.
        notify.notify_waiters();
    }

    #[tokio::test]
    async fn register_session_attaches_idle_publish_token_hint() {
        let state = AppState::new().with_idle_snapshot_interval(Some(5000));
        let register = RegisterSessionRequest {
            session_id: "sess-publish".into(),
            private_beach_id: "pb-publish".into(),
            harness_type: HarnessType::TerminalShim,
            capabilities: vec![],
            location_hint: None,
            metadata: None,
            version: "1.0.0".into(),
            viewer_passcode: Some("code-123".into()),
            transport_mode: Some(TransportMode::FastPath),
        };
        let resp = state.register_session(register).await.unwrap();
        let publish_hint = resp
            .transport_hints
            .get(IDLE_PUBLISH_TOKEN_HINT_KEY)
            .expect("publish hint stored on transport_hints");
        assert!(publish_hint.get("token").and_then(|v| v.as_str()).is_some());
        let idle_snapshot = resp
            .transport_hints
            .get("idle_snapshot")
            .expect("idle snapshot hint present");
        let nested = idle_snapshot
            .get("publish_token")
            .and_then(|value| value.get("token"))
            .and_then(|value| value.as_str());
        assert!(nested.is_some(), "idle snapshot hint should embed token");
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

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            EnvVarGuard { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[derive(Clone)]
    struct TestTransport {
        id: TransportId,
        peer: TransportId,
        kind: TransportKind,
    }

    impl TestTransport {
        fn new(id: u64, kind: TransportKind) -> Self {
            Self {
                id: TransportId(id),
                peer: TransportId(id + 1),
                kind,
            }
        }
    }

    impl Transport for TestTransport {
        fn kind(&self) -> TransportKind {
            self.kind
        }

        fn id(&self) -> TransportId {
            self.id
        }

        fn peer(&self) -> TransportId {
            self.peer
        }

        fn send(&self, _message: TransportMessage) -> Result<(), TransportError> {
            Ok(())
        }

        fn send_text(&self, _text: &str) -> Result<u64, TransportError> {
            Ok(0)
        }

        fn send_bytes(&self, _bytes: &[u8]) -> Result<u64, TransportError> {
            Ok(0)
        }

        fn recv(&self, _timeout: StdDuration) -> Result<TransportMessage, TransportError> {
            Err(TransportError::Timeout)
        }

        fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
            Ok(None)
        }
    }

    /// Transport that accepts sends but never delivers acks (recv always times out).
    #[derive(Clone)]
    struct StallingTransport {
        id: TransportId,
        peer: TransportId,
        kind: TransportKind,
    }

    impl StallingTransport {
        fn new(id: u64, kind: TransportKind) -> Self {
            Self {
                id: TransportId(id),
                peer: TransportId(id + 1),
                kind,
            }
        }
    }

    /// Transport that fails immediately on send to simulate channel failures.
    #[derive(Clone)]
    struct FailingTransport {
        id: TransportId,
        peer: TransportId,
        kind: TransportKind,
        fail_on_send: bool,
        closed: Arc<AtomicBool>,
        close_on_recv: bool,
        sent: Arc<AtomicUsize>,
    }

    impl FailingTransport {
        fn new(id: u64, kind: TransportKind) -> Self {
            Self {
                id: TransportId(id),
                peer: TransportId(id + 1),
                kind,
                fail_on_send: true,
                closed: Arc::new(AtomicBool::new(false)),
                close_on_recv: false,
                sent: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn new_closed_on_recv(id: u64, kind: TransportKind) -> Self {
            Self {
                id: TransportId(id),
                peer: TransportId(id + 1),
                kind,
                fail_on_send: false,
                closed: Arc::new(AtomicBool::new(true)),
                close_on_recv: true,
                sent: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn send_count(&self) -> usize {
            self.sent.load(Ordering::SeqCst)
        }
    }

    impl Transport for FailingTransport {
        fn kind(&self) -> TransportKind {
            self.kind
        }

        fn id(&self) -> TransportId {
            self.id
        }

        fn peer(&self) -> TransportId {
            self.peer
        }

        fn send(&self, _message: TransportMessage) -> Result<(), TransportError> {
            if self.fail_on_send {
                self.closed.store(true, Ordering::SeqCst);
                Err(TransportError::ChannelClosed)
            } else {
                self.sent.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        fn send_text(&self, _text: &str) -> Result<u64, TransportError> {
            if self.fail_on_send {
                self.closed.store(true, Ordering::SeqCst);
                Err(TransportError::ChannelClosed)
            } else {
                self.sent.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }
        }

        fn send_bytes(&self, _bytes: &[u8]) -> Result<u64, TransportError> {
            if self.fail_on_send {
                self.closed.store(true, Ordering::SeqCst);
                Err(TransportError::ChannelClosed)
            } else {
                self.sent.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }
        }

        fn recv(&self, _timeout: StdDuration) -> Result<TransportMessage, TransportError> {
            if self.closed.load(Ordering::SeqCst) || self.close_on_recv {
                self.closed.store(true, Ordering::SeqCst);
                Err(TransportError::ChannelClosed)
            } else {
                Err(TransportError::Timeout)
            }
        }

        fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
            Ok(None)
        }
    }

    impl Transport for StallingTransport {
        fn kind(&self) -> TransportKind {
            self.kind
        }

        fn id(&self) -> TransportId {
            self.id
        }

        fn peer(&self) -> TransportId {
            self.peer
        }

        fn send(&self, _message: TransportMessage) -> Result<(), TransportError> {
            Ok(())
        }

        fn send_text(&self, _text: &str) -> Result<u64, TransportError> {
            Ok(0)
        }

        fn send_bytes(&self, _bytes: &[u8]) -> Result<u64, TransportError> {
            Ok(0)
        }

        fn recv(&self, _timeout: StdDuration) -> Result<TransportMessage, TransportError> {
            Err(TransportError::Timeout)
        }

        fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn unified_forwarder_ack_stall_does_not_downgrade_transport_status() {
        let state = Arc::new(AppState::new().with_controller_strict_gating(false));
        let session_id = "sess-unified-stall";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
            record.mark_http_ready();
            record.transport_mode = TransportMode::FastPath;
        })
        .await;

        // Unified-only: fast-path enabled, but no legacy WebRTC channels are present.
        let primary = Arc::new(StallingTransport::new(200, TransportKind::WebSocket));
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let state_for_forwarder = Arc::clone(&state);
        let forwarder = tokio::spawn(async move {
            drive_controller_forwarder(
                state_for_forwarder.as_ref(),
                "pb-test",
                session_id,
                primary,
                None,
                None,
                None,
                cancel_for_task,
            )
            .await
            .ok();
        });

        state
            .queue_actions(session_id, &token, vec![new_action("stall-1")], None)
            .await
            .expect("actions should queue in unified fast-path mode");

        // Wait out the ack-stall deadline (1.5s) and ensure status stays fast_path.
        tokio::time::sleep(StdDuration::from_millis(1800)).await;

        let status = state
            .fallback
            .peek_pending_transport_status(session_id)
            .await
            .expect("transport status should be cached");
        assert_eq!(status.transport, PairingTransportKind::Rtc);
        assert!(state.is_rtc_ready(session_id).await);

        // Shutdown forwarder cleanly.
        cancel.cancel();
        let _ = forwarder.await;
    }

    #[test_timeout::tokio_timeout_test(10)]
    async fn unified_forwarder_send_failure_stays_fast_path_status() {
        let state = Arc::new(AppState::new().with_controller_strict_gating(false));
        let session_id = "sess-unified-sendfail";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
            record.mark_http_ready();
            record.transport_mode = TransportMode::FastPath;
        })
        .await;

        // Unified-only transport that fails send calls with ChannelClosed.
        let primary = Arc::new(FailingTransport::new(300, TransportKind::WebSocket));
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let state_for_forwarder = Arc::clone(&state);
        let forwarder = tokio::spawn(async move {
            drive_controller_forwarder(
                state_for_forwarder.as_ref(),
                "pb-test",
                session_id,
                primary,
                None,
                None,
                None,
                cancel_for_task,
            )
            .await
            .ok();
        });

        state
            .queue_actions(session_id, &token, vec![new_action("fail-1")], None)
            .await
            .expect("actions should queue in unified fast-path mode");

        // Allow dispatch loop to attempt send and handle the failure.
        tokio::time::sleep(StdDuration::from_millis(300)).await;

        let status = state
            .fallback
            .peek_pending_transport_status(session_id)
            .await
            .expect("transport status should be cached");
        assert_eq!(status.transport, PairingTransportKind::Rtc);
        assert!(state.is_rtc_ready(session_id).await);

        cancel.cancel();
        let _ = forwarder.await;
    }

    #[test_timeout::tokio_timeout_test(10)]
    async fn unified_forwarder_shutdown_does_not_downgrade_status() {
        let state = Arc::new(AppState::new().with_controller_strict_gating(false));
        let session_id = "sess-unified-shutdown";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
            record.mark_http_ready();
            record.transport_mode = TransportMode::FastPath;
        })
        .await;

        // Transport that will report ChannelClosed on recv after we simulate closure.
        let primary = Arc::new(FailingTransport::new_closed_on_recv(
            400,
            TransportKind::WebSocket,
        ));
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let state_for_forwarder = Arc::clone(&state);
        let forwarder = tokio::spawn(async move {
            drive_controller_forwarder(
                state_for_forwarder.as_ref(),
                "pb-test",
                session_id,
                primary,
                None,
                None,
                None,
                cancel_for_task,
            )
            .await
            .ok();
        });

        state
            .queue_actions(session_id, &token, vec![new_action("shutdown-1")], None)
            .await
            .expect("actions should queue in unified fast-path mode");

        // Give the forwarder loop time to hit recv and observe closure.
        tokio::time::sleep(StdDuration::from_millis(400)).await;

        let status = state
            .fallback
            .peek_pending_transport_status(session_id)
            .await
            .expect("transport status should be cached");
        assert_eq!(status.transport, PairingTransportKind::Rtc);
        assert!(state.is_rtc_ready(session_id).await);

        cancel.cancel();
        let _ = forwarder.await;
    }
    #[test]
    fn session_record_applies_controller_auto_attach_hint() {
        let mut record = SessionRecord::new(
            "sess-auto",
            "pb-auto",
            &HarnessType::Custom,
            None,
            TransportMode::FastPath,
        );
        let hint = ControllerAutoAttachHint {
            private_beach_id: "pb-auto".into(),
            attach_code: "ABC123".into(),
            manager_url: "http://localhost:8080".into(),
            issued_at: Utc::now(),
            expires_at: None,
            handshake_id: None,
        };
        record
            .upsert_controller_auto_attach_hint(&hint)
            .expect("controller hint should serialize");
        let stored = record
            .transport_hints
            .get("controller_auto_attach")
            .and_then(|value| value.as_object())
            .expect("controller hint missing");
        assert_eq!(
            stored.get("private_beach_id").and_then(|v| v.as_str()),
            Some("pb-auto")
        );
        assert_eq!(
            stored.get("attach_code").and_then(|v| v.as_str()),
            Some("ABC123")
        );
        assert_eq!(
            stored.get("manager_url").and_then(|v| v.as_str()),
            Some("http://localhost:8080")
        );
    }

    #[test_timeout::tokio_timeout_test(10)]
    async fn controller_commands_require_ready_session() {
        let state = AppState::new().with_controller_strict_gating(true);
        let session_id = "sess-ready";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
        })
        .await;

        {
            let mut sessions = state.fallback.sessions.write().await;
            if let Some(record) = sessions.get_mut(session_id) {
                record.mark_http_ready();
                record.last_health_at = Some(Instant::now());
            }
        }

        state
            .queue_actions(session_id, &token, vec![new_action("cmd-ready-2")], None)
            .await
            .expect("commands accepted once ready");
    }

    #[test_timeout::tokio_timeout_test(10)]
    async fn controller_commands_reject_missing_lease() {
        let state = AppState::new().with_controller_strict_gating(true);
        let session_id = Uuid::new_v4().to_string();
        let private_beach_id = Uuid::new_v4().to_string();
        state
            .attach_owned(&private_beach_id, vec![session_id.clone()], None)
            .await
            .expect("attach_owned succeeds");

        let bogus_token = Uuid::new_v4().to_string();
        let cmd = ActionCommand {
            id: "cmd-missing-lease".into(),
            action_type: "key".into(),
            payload: serde_json::json!({ "key": "y" }),
            expires_at: None,
        };

        let err = state
            .queue_actions(&session_id, &bogus_token, vec![cmd], None)
            .await
            .expect_err("invalid lease should be rejected");

        match err {
            StateError::ControllerCommandRejected { reason } => {
                assert_eq!(reason, ControllerCommandDropReason::MissingLease);
            }
            other => panic!("expected missing lease rejection, got {other:?}"),
        }
    }

    #[test_timeout::tokio_timeout_test(10)]
    async fn http_fallback_mode_enqueues_actions() {
        let state = AppState::new().with_controller_strict_gating(false);
        let session_id = "sess-http-fallback";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
            record.mark_http_ready();
            record.transport_mode = TransportMode::HttpFallback;
        })
        .await;

        state
            .queue_actions(session_id, &token, vec![new_action("cmd-http")], None)
            .await
            .expect("http fallback mode should enqueue actions");

        let sessions = state.fallback.sessions.read().await;
        let record = sessions
            .get(session_id)
            .expect("session stored in fallback");
        assert_eq!(record.pending_actions.len(), 1);
    }

    async fn insert_manual_session<F>(state: &AppState, session_id: &str, configure: F) -> String
    where
        F: FnOnce(&mut SessionRecord),
    {
        let mut sessions = state.fallback.sessions.write().await;
        let mut record = SessionRecord::new(
            session_id,
            "pb-test",
            &HarnessType::Custom,
            None,
            TransportMode::FastPath,
        );
        let token = Uuid::new_v4().to_string();
        record.ensure_lease(
            token.clone(),
            now_ms() + 60_000,
            None,
            None,
            Some("test".into()),
        );
        configure(&mut record);
        sessions.insert(session_id.to_string(), record);
        token
    }

    fn new_action(id: &str) -> ActionCommand {
        ActionCommand {
            id: id.into(),
            action_type: "key".into(),
            payload: json!({ "key": "w" }),
            expires_at: None,
        }
    }

    fn drop_metric(reason: ControllerCommandDropReason) -> u64 {
        metrics::CONTROLLER_ACTIONS_DROPPED
            .with_label_values(&[reason.code()])
            .get()
    }

    #[tokio::test]
    async fn controller_commands_reject_child_not_attached_state() {
        let state = AppState::new().with_controller_strict_gating(true);
        let session_id = "child-not-attached";
        let token = insert_manual_session(&state, session_id, |_| {}).await;
        let before = drop_metric(ControllerCommandDropReason::ChildNotAttached);
        let err = state
            .queue_actions(session_id, &token, vec![new_action("cmd-attach")], None)
            .await
            .expect_err("unattached session should reject");
        match err {
            StateError::ControllerCommandRejected { reason } => {
                assert_eq!(reason, ControllerCommandDropReason::ChildNotAttached);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let after = drop_metric(ControllerCommandDropReason::ChildNotAttached);
        assert_eq!(after, before + 1);
    }

    #[tokio::test]
    async fn controller_commands_reject_child_offline_state() {
        let state = AppState::new().with_controller_strict_gating(true);
        let session_id = "child-offline";
        let token = insert_manual_session(&state, session_id, |record| {
            record.mark_attached();
            record.mark_http_ready();
            record.last_health_at =
                Some(Instant::now() - (STALE_SESSION_MAX_IDLE + StdDuration::from_secs(5)));
        })
        .await;
        let before = drop_metric(ControllerCommandDropReason::ChildOffline);
        let err = state
            .queue_actions(session_id, &token, vec![new_action("cmd-offline")], None)
            .await
            .expect_err("offline session should reject");
        match err {
            StateError::ControllerCommandRejected { reason } => {
                assert_eq!(reason, ControllerCommandDropReason::ChildOffline);
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let after = drop_metric(ControllerCommandDropReason::ChildOffline);
        assert_eq!(after, before + 1);
    }
}

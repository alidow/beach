pub mod join;

use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, StyleId, unpack_cell};
use crate::client::grid_renderer::{GridRenderer, SelectionMode, SelectionPosition};
use crate::protocol::{
    self, ClientFrame as WireClientFrame, CursorFrame, FEATURE_CURSOR_SYNC,
    HostFrame as WireHostFrame, Update as WireUpdate, ViewportCommand,
};
use crate::telemetry::{self, PerfGuard};
use crate::transport::{Payload, Transport, TransportError};
#[cfg(not(test))]
use copypasta::{ClipboardContext, ClipboardProvider};
use crossterm::{
    cursor::{MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::cmp;
use std::collections::HashMap;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::sync::{
    Arc,
    mpsc::{Receiver, TryRecvError},
};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{Level, debug, trace};

const BACKFILL_LOOKAHEAD_ROWS: usize = 120;
const BACKFILL_MAX_ROWS_PER_REQUEST: u32 = 256;
const BACKFILL_MAX_PENDING_REQUESTS: usize = 4;
const BACKFILL_MIN_INTERVAL: Duration = Duration::from_millis(250);
const BACKFILL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MOUSE_SCROLL_LINES: isize = 5;
const COPY_MODE_KEYSET_ENV: &str = "BEACH_COPY_MODE_KEYS";
const TMUX_PREFIX_TIMEOUT: Duration = Duration::from_millis(500);
const AUTH_SPINNER_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
const AUTH_SPINNER_INTERVAL: Duration = Duration::from_millis(120);
const AUTH_APPROVED_MESSAGE_DURATION: Duration = Duration::from_millis(1200);
const AUTH_DENIED_EXIT_DELAY: Duration = Duration::from_millis(1200);
const AUTH_FALLBACK_WAIT: Duration = Duration::from_millis(750);
const AUTH_HINT_STAGE_ONE: Duration = Duration::from_secs(10);
const AUTH_HINT_STAGE_TWO: Duration = Duration::from_secs(30);
const AUTH_WAIT_MESSAGE: &str = "Waiting for host approval...";
const AUTH_WAIT_MESSAGE_INIT: &str = "Connected - waiting for host approval...";
const AUTH_WAIT_MESSAGE_SYNCING: &str = "Connected - syncing remote session...";
const AUTH_WAIT_HINT_ONE: &str = "Still waiting... hang tight.";
const AUTH_WAIT_HINT_TWO: &str = "Still waiting... ask the host to approve.";
const AUTH_APPROVED_MESSAGE: &str = "Approved - syncing...";
const AUTH_DENIED_MESSAGE: &str = "Join request was declined by host.";
const AUTH_DISCONNECTED_MESSAGE: &str = "Disconnected before approval.";

const PREDICTION_SRTT_TRIGGER_LOW_MS: f64 = 20.0;
const PREDICTION_SRTT_TRIGGER_HIGH_MS: f64 = 30.0;
const PREDICTION_FLAG_TRIGGER_LOW_MS: f64 = 50.0;
const PREDICTION_FLAG_TRIGGER_HIGH_MS: f64 = 80.0;
const PREDICTION_GLITCH_THRESHOLD: Duration = Duration::from_millis(250);
const PREDICTION_GLITCH_REPAIR_COUNT: u32 = 10;
const PREDICTION_GLITCH_REPAIR_MIN_INTERVAL: Duration = Duration::from_millis(150);
const PREDICTION_GLITCH_FLAG_THRESHOLD: Duration = Duration::from_millis(5000);
const PREDICTION_SRTT_ALPHA: f64 = 0.125;
const PREDICTION_ACK_GRACE: Duration = Duration::from_millis(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorizationState {
    Connecting,
    Waiting,
    Approved,
    Denied,
}

#[cfg(test)]
mod clipboard {
    #[allow(unused_imports)]
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static TEST_CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    pub fn set(contents: &str) -> Result<(), String> {
        TEST_CLIPBOARD.with(|cell| {
            *cell.borrow_mut() = Some(contents.to_string());
        });
        Ok(())
    }

    pub fn get() -> Result<String, String> {
        TEST_CLIPBOARD.with(|cell| {
            cell.borrow()
                .clone()
                .ok_or_else(|| "clipboard empty".to_string())
        })
    }

    pub fn clear() {
        TEST_CLIPBOARD.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

#[cfg(not(test))]
mod clipboard {
    #[allow(unused_imports)]
    use super::*;

    pub fn set(contents: &str) -> Result<(), String> {
        let mut ctx = ClipboardContext::new().map_err(|err| err.to_string())?;
        ctx.set_contents(contents.to_string())
            .map_err(|err| err.to_string())
    }

    pub fn get() -> Result<String, String> {
        let mut ctx = ClipboardContext::new().map_err(|err| err.to_string())?;
        ctx.get_contents().map_err(|err| err.to_string())
    }

    #[allow(dead_code)]
    pub fn clear() {}
}

use clipboard::{get as clipboard_get, set as clipboard_set};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyModeKeySet {
    Vi,
    Emacs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyModeSearchDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WordMotion {
    NextStart,
    NextEnd,
    PrevStart,
}

#[derive(Clone, Debug)]
enum CopyModePendingInput {
    Search {
        direction: CopyModeSearchDirection,
        buffer: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardWordKind {
    Start,
    End,
}

#[derive(Clone, Debug)]
struct CopyModeSearch {
    direction: CopyModeSearchDirection,
    pattern: String,
}

#[derive(Clone, Copy, Debug)]
enum CopyModeCommand {
    Move { rows: isize, cols: isize },
    MoveToLineStart,
    MoveToLineEnd,
    Page { delta: isize },
    HalfPage { delta: isize },
    JumpTop,
    JumpBottom,
    MoveWord(WordMotion),
    BeginSelection,
    ClearSelection,
    ToggleSelection,
    SetSelectionMode(SelectionMode),
    CopySelection,
    CopySelectionAndExit,
    Cancel,
    SetMode(CopyModeKeySet),
    Search(CopyModeSearchDirection),
    RepeatLastSearch(CopyModeSearchDirection),
}

#[derive(Clone, Debug)]
struct BackfillRequestState {
    id: u64,
    start: u64,
    end: u64,
    issued_at: Instant,
    more_expected: bool,
}

#[derive(Clone, Debug)]
struct EmptyTailRange {
    start: u64,
    end: u64,
    recorded_at: Instant,
    highest_at: Option<u64>,
    retry_attempted: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(TransportError),
    #[error("protocol error: {0}")]
    Protocol(#[from] protocol::WireError),
    #[error("shutdown requested")]
    Shutdown,
}

pub struct TerminalClient {
    transport: Arc<dyn Transport>,
    renderer: GridRenderer,
    render_enabled: bool,
    tui: Option<Terminal<CrosstermBackend<io::Stdout>>>,
    last_seq: Seq,
    input_rx: Option<Receiver<Vec<u8>>>,
    input_seq: Seq,
    force_render: bool,
    cursor_row: usize,
    cursor_col: usize,
    cursor_seq: Seq,
    cursor_support: bool,
    cursor_authoritative: bool,
    cursor_authoritative_pending: bool,
    cursor_visible: bool,
    pending_predictions: HashMap<Seq, PendingPrediction>,
    prediction_srtt_ms: Option<f64>,
    prediction_srtt_trigger: bool,
    prediction_flagging: bool,
    prediction_glitch_trigger: u32,
    prediction_last_quick_confirmation: Option<Instant>,
    copy_mode: Option<CopyModeState>,
    last_render_at: Option<Instant>,
    render_interval: Duration,
    pending_render: bool,
    predictive_input: bool,
    forward_mouse_to_host: bool,
    tmux_prefix_started_at: Option<Instant>,
    subscription_id: Option<u64>,
    handshake_history_rows: u64,
    handshake_snapshot_lines: u32,
    next_backfill_request_id: u64,
    pending_backfills: Vec<BackfillRequestState>,
    last_backfill_request_at: Option<Instant>,
    known_base_row: Option<u64>,
    has_loaded_rows: bool,
    highest_loaded_row: Option<u64>,
    last_tail_backfill_start: Option<u64>,
    last_gap_backfill_start: Option<u64>,
    empty_tail_ranges: Vec<EmptyTailRange>,
    last_backfill_trimmed: bool,
    authorization_state: AuthorizationState,
    authorization_message: Option<String>,
    authorization_wait_started: Option<Instant>,
    authorization_last_tick: Instant,
    authorization_spinner_index: usize,
    authorization_approved_since: Option<Instant>,
    authorization_shutdown_at: Option<Instant>,
    authorization_hint_level: u8,
    connect_started_at: Instant,
    authorization_pending_hint: bool,
}

impl TerminalClient {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        let render_enabled = io::stdout().is_terminal();
        let mut renderer = GridRenderer::new(0, 0);
        renderer.on_resize(80, 24);
        renderer.set_predictions_visible(false);
        renderer.set_prediction_flagging(false);
        Self {
            transport,
            renderer,
            render_enabled,
            tui: None,
            last_seq: 0,
            input_rx: None,
            input_seq: 0,
            force_render: true,
            cursor_row: 0,
            cursor_col: 0,
            cursor_seq: 0,
            cursor_support: false,
            cursor_authoritative: false,
            cursor_authoritative_pending: false,
            cursor_visible: true,
            pending_predictions: HashMap::new(),
            prediction_srtt_ms: None,
            prediction_srtt_trigger: false,
            prediction_flagging: false,
            prediction_glitch_trigger: 0,
            prediction_last_quick_confirmation: None,
            copy_mode: None,
            last_render_at: None,
            render_interval: Duration::from_millis(16),
            pending_render: false,
            predictive_input: false,
            forward_mouse_to_host: false,
            tmux_prefix_started_at: None,
            subscription_id: None,
            handshake_history_rows: 0,
            handshake_snapshot_lines: 0,
            next_backfill_request_id: 1,
            pending_backfills: Vec::new(),
            last_backfill_request_at: None,
            known_base_row: None,
            has_loaded_rows: false,
            highest_loaded_row: None,
            last_tail_backfill_start: None,
            last_gap_backfill_start: None,
            empty_tail_ranges: Vec::new(),
            last_backfill_trimmed: false,
            authorization_state: AuthorizationState::Connecting,
            authorization_message: None,
            authorization_wait_started: None,
            authorization_last_tick: Instant::now(),
            authorization_spinner_index: 0,
            authorization_approved_since: None,
            authorization_shutdown_at: None,
            authorization_hint_level: 0,
            connect_started_at: Instant::now(),
            authorization_pending_hint: false,
        }
    }

    pub fn with_render(mut self, enabled: bool) -> Self {
        self.render_enabled = enabled;
        self
    }

    pub fn with_input(mut self, rx: Receiver<Vec<u8>>) -> Self {
        self.input_rx = Some(rx);
        self
    }

    pub fn with_predictive_input(mut self, enabled: bool) -> Self {
        self.predictive_input = enabled;
        self.reset_prediction_state();
        self
    }

    #[cfg(test)]
    pub fn renderer_base_row(&self) -> u64 {
        self.renderer.base_row()
    }

    #[cfg(test)]
    pub fn known_base_row(&self) -> Option<u64> {
        self.known_base_row
    }
    pub fn run(mut self) -> Result<(), ClientError> {
        self.setup_tui()?;
        debug!(target = "client::loop", "client loop started");

        let run_result = (|| -> Result<(), ClientError> {
            loop {
                self.pump_input()?;
                self.maybe_request_backfill()?;
                self.tick_authorization();
                self.update_prediction_overlay();
                let message = match self.transport.recv(Duration::from_millis(25)) {
                    Ok(message) => Some(message),
                    Err(TransportError::Timeout) => None,
                    Err(TransportError::ChannelClosed) => {
                        if self.subscription_id.is_none() {
                            self.set_authorization_state(
                                AuthorizationState::Denied,
                                Some(AUTH_DISCONNECTED_MESSAGE.to_string()),
                            );
                            self.refresh_authorization_status();
                            self.force_render = true;
                            self.maybe_render()?;
                            thread::sleep(AUTH_DENIED_EXIT_DELAY);
                        }
                        return Ok(());
                    }
                    Err(err) => return Err(ClientError::Transport(err)),
                };

                if let Some(message) = message {
                    match message.payload {
                        Payload::Binary(bytes) => {
                            telemetry::record_bytes("client_frame_bytes", bytes.len());
                            let decode_start = Instant::now();
                            let frame = protocol::decode_host_frame_binary(&bytes)?;
                            let decode_elapsed = decode_start.elapsed();
                            match &frame {
                                WireHostFrame::Snapshot { .. } => telemetry::record_duration(
                                    "client_decode_snapshot",
                                    decode_elapsed,
                                ),
                                WireHostFrame::Delta { .. } => telemetry::record_duration(
                                    "client_decode_delta",
                                    decode_elapsed,
                                ),
                                _ => telemetry::record_duration(
                                    "client_decode_frame",
                                    decode_elapsed,
                                ),
                            }
                            match self.handle_host_frame(frame) {
                                Ok(()) => {}
                                Err(ClientError::Shutdown) => return Ok(()),
                                Err(err) => return Err(err),
                            }
                            self.maybe_request_backfill()?;
                        }
                        Payload::Text(text) => {
                            telemetry::record_bytes("client_frame_bytes", text.len());
                            let trimmed = text.trim();
                            if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                                trace!(
                                    target = "client::frame",
                                    payload = trimmed,
                                    "ignoring handshake sentinel"
                                );
                            } else if self.handle_authorization_signal(trimmed) {
                                trace!(target = "client::frame", payload = %trimmed, "processed authorization signal");
                            } else {
                                debug!(
                                    target = "client::frame",
                                    payload = %trimmed,
                                    "unexpected text payload"
                                );
                            }
                        }
                    }
                }

                self.maybe_render()?;
                if matches!(self.authorization_state, AuthorizationState::Denied) {
                    if let Some(deadline) = self.authorization_shutdown_at {
                        if Instant::now() >= deadline {
                            return Ok(());
                        }
                    }
                }
            }
        })();

        let teardown_result = self.teardown_tui();
        debug!(target = "client::loop", "client loop stopped");

        match (run_result, teardown_result) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    fn handle_host_frame(&mut self, frame: WireHostFrame) -> Result<(), ClientError> {
        if tracing::enabled!(Level::DEBUG) {
            let frame_type = match &frame {
                WireHostFrame::Heartbeat { .. } => "heartbeat",
                WireHostFrame::Hello { .. } => "hello",
                WireHostFrame::Grid { .. } => "grid",
                WireHostFrame::Snapshot { .. } => "snapshot",
                WireHostFrame::SnapshotComplete { .. } => "snapshot_complete",
                WireHostFrame::Delta { .. } => "delta",
                WireHostFrame::HistoryBackfill { .. } => "history_backfill",
                WireHostFrame::InputAck { .. } => "input_ack",
                WireHostFrame::Cursor { .. } => "cursor",
                WireHostFrame::Shutdown => "shutdown",
            };
            debug!(
                target = "client::frame",
                frame = frame_type,
                authorization_state = ?self.authorization_state,
                "processing binary frame"
            );
        }

        let _guard = PerfGuard::new("client_handle_frame_binary");
        match frame {
            WireHostFrame::Heartbeat { .. } => {}
            WireHostFrame::Hello {
                subscription,
                max_seq,
                config,
                features,
            } => {
                self.subscription_id = Some(subscription);
                self.last_seq = cmp::max(self.last_seq, max_seq);
                self.cursor_support = (features & FEATURE_CURSOR_SYNC) != 0;
                self.cursor_authoritative = false;
                self.cursor_authoritative_pending = false;
                self.cursor_seq = 0;
                self.cursor_visible = true;
                self.renderer.clear_cursor();
                self.sync_renderer_cursor();
                self.pending_backfills.clear();
                self.next_backfill_request_id = 1;
                self.last_backfill_request_at = None;
                self.known_base_row = None;
                self.has_loaded_rows = false;
                self.highest_loaded_row = None;
                self.last_tail_backfill_start = None;
                self.last_gap_backfill_start = None;
                self.empty_tail_ranges.clear();
                self.last_backfill_trimmed = false;
                self.handshake_snapshot_lines = config.initial_snapshot_lines;
                self.handshake_history_rows = 0;
                debug!(
                    subscription = subscription,
                    initial_snapshot_lines = config.initial_snapshot_lines,
                    "received Hello frame, setting Approved state"
                );
                self.set_authorization_state(
                    AuthorizationState::Approved,
                    Some(AUTH_APPROVED_MESSAGE.to_string()),
                );
            }
            WireHostFrame::Grid {
                cols,
                history_rows,
                base_row: _,
                viewport_rows,
            } => {
                let local_viewport = self.renderer.viewport_height().max(1);
                let visible_rows = if local_viewport > 0 {
                    local_viewport
                } else {
                    viewport_rows.map(|rows| rows.max(1) as usize).unwrap_or(1)
                };
                trace!(
                    target = "client::render",
                    server_viewport = ?viewport_rows,
                    local_viewport,
                    visible_rows,
                    cols,
                    history_rows,
                    "grid handshake"
                );
                let total_rows = history_rows.max(visible_rows as u32) as usize;
                let cols = cols as usize;
                self.renderer.ensure_size(total_rows, cols);
                self.renderer.mark_dirty();
                self.force_render = true;
                self.cursor_row = visible_rows.saturating_sub(1);
                self.cursor_col = 0;
                self.renderer.clear_all_predictions();
                self.pending_predictions.clear();
                self.reset_prediction_state();
                self.handshake_history_rows = history_rows as u64;
            }
            WireHostFrame::Snapshot {
                updates,
                watermark,
                cursor,
                ..
            } => {
                if cursor.is_some() && self.cursor_support {
                    self.cursor_authoritative_pending = true;
                }
                for update in &updates {
                    self.observe_update_bounds(update, true);
                    let (update_kind, row_hint, seq_hint) = Self::update_debug_metadata(update);
                    self.renderer.set_debug_update_context(
                        "snapshot",
                        update_kind,
                        row_hint,
                        seq_hint,
                    );
                    self.apply_wire_update(update);
                    self.renderer.clear_debug_update_context();
                }
                if let Some(cursor_frame) = cursor {
                    self.apply_wire_cursor(&cursor_frame);
                }
                self.last_seq = cmp::max(self.last_seq, watermark);
                self.force_render = true;
            }
            WireHostFrame::Delta {
                updates,
                watermark,
                cursor,
                ..
            } => {
                if cursor.is_some() && self.cursor_support {
                    self.cursor_authoritative_pending = true;
                }
                for update in &updates {
                    self.observe_update_bounds(update, false);
                    let (update_kind, row_hint, seq_hint) = Self::update_debug_metadata(update);
                    self.renderer.set_debug_update_context(
                        "delta",
                        update_kind,
                        row_hint,
                        seq_hint,
                    );
                    self.apply_wire_update(update);
                    self.renderer.clear_debug_update_context();
                }
                if let Some(cursor_frame) = cursor {
                    self.apply_wire_cursor(&cursor_frame);
                }
                self.last_seq = cmp::max(self.last_seq, watermark);
                self.force_render = true;
            }
            WireHostFrame::HistoryBackfill {
                subscription,
                request_id,
                start_row,
                count,
                updates,
                more,
                cursor,
            } => {
                if cursor.is_some() && self.cursor_support {
                    self.cursor_authoritative_pending = true;
                }
                for update in &updates {
                    self.observe_update_bounds(update, true);
                }
                self.handle_history_backfill(
                    subscription,
                    request_id,
                    start_row,
                    count,
                    updates,
                    more,
                )?;
                if let Some(cursor_frame) = cursor {
                    self.apply_wire_cursor(&cursor_frame);
                }
            }
            WireHostFrame::InputAck { seq } => {
                self.handle_input_ack(seq);
            }
            WireHostFrame::Cursor { cursor, .. } => {
                self.apply_wire_cursor(&cursor);
            }
            WireHostFrame::SnapshotComplete { .. } => {
                debug!(
                    authorization_state = ?self.authorization_state,
                    has_loaded_rows = self.has_loaded_rows,
                    "received SnapshotComplete"
                );
            }
            WireHostFrame::Shutdown => return Err(ClientError::Shutdown),
        }
        Ok(())
    }

    fn observe_update_bounds(&mut self, update: &WireUpdate, authoritative: bool) {
        let min_row = match update {
            WireUpdate::Cell { row, .. }
            | WireUpdate::Row { row, .. }
            | WireUpdate::RowSegment { row, .. } => Some(*row as u64),
            WireUpdate::Rect { rows, .. } => rows.first().map(|r| *r as u64),
            WireUpdate::Trim { .. } => None,
            WireUpdate::Style { .. } => None,
        };
        if let Some(row) = min_row {
            if authoritative {
                let base = self.known_base_row.map_or(row, |current| current.min(row));
                if Some(base) != self.known_base_row {
                    self.known_base_row = Some(base);
                }
                self.renderer.set_base_row(base);
                trace!(
                    target = "client::render",
                    row,
                    base,
                    known_base = ?self.known_base_row,
                    "authoritative bounds"
                );
            } else if row < self.renderer.base_row() {
                self.renderer.set_base_row(row);
                trace!(
                    target = "client::render",
                    row,
                    base_row = self.renderer.base_row(),
                    "non-authoritative bounds"
                );
            }
        }
    }

    fn sync_renderer_cursor(&mut self) {
        let visible = if self.cursor_support {
            if self.cursor_authoritative || self.cursor_authoritative_pending {
                self.cursor_visible
            } else {
                true
            }
        } else {
            true
        };
        self.renderer
            .set_cursor(self.cursor_row as u64, self.cursor_col, visible);
    }

    fn note_loaded_row(&mut self, row: u64) {
        self.highest_loaded_row = Some(match self.highest_loaded_row {
            Some(existing) => existing.max(row),
            None => row,
        });
    }

    fn prune_backfill_requests(&mut self) {
        let mut removed = false;
        self.pending_backfills.retain(|req| {
            if req.issued_at.elapsed() > BACKFILL_REQUEST_TIMEOUT {
                removed = true;
                false
            } else {
                true
            }
        });
        if removed {
            self.last_backfill_request_at = None;
        }
    }

    fn maybe_request_backfill(&mut self) -> Result<(), ClientError> {
        let subscription = match self.subscription_id {
            Some(id) => id,
            None => return Ok(()),
        };
        self.prune_backfill_requests();
        if !self.has_loaded_rows {
            return Ok(());
        }

        if self.renderer.is_following_tail()
            && self.pending_backfills.is_empty()
            && self.last_backfill_request_at.is_none()
            && self.next_backfill_request_id == 1
            && (self.renderer.total_rows() > self.renderer.viewport_height() as u64
                || self.renderer.has_pending_rows()
                || self.renderer.has_missing_rows())
            && self.handshake_history_rows > self.handshake_snapshot_lines as u64
        {
            if let Some((start, span)) = self.renderer.first_unloaded_range(BACKFILL_LOOKAHEAD_ROWS)
            {
                let count = span.min(BACKFILL_MAX_ROWS_PER_REQUEST);
                if count > 0 {
                    let end = start.saturating_add(count as u64);
                    let overlaps_trimmed = self.empty_tail_ranges.iter().any(|range| {
                        range.retry_attempted
                            && Self::ranges_overlap(start, end, range.start, range.end)
                    });
                    let should_defer = self.should_defer_empty_retry(start, end);
                    if !overlaps_trimmed && !should_defer && !self.is_range_pending(start, end) {
                        self.send_backfill_request(subscription, start, count)?;
                        self.last_backfill_request_at = Some(Instant::now());
                    }
                }
            }
            return Ok(());
        }

        // Disable all backfill activity while following the live tail unless we detect gaps.

        if self.pending_backfills.is_empty() {
            if !self.renderer.is_following_tail() {
                if let (Some(base), Some(highest)) = (self.known_base_row, self.highest_loaded_row)
                {
                    if base < highest
                        && highest.saturating_sub(base) > BACKFILL_LOOKAHEAD_ROWS as u64
                    {
                        if let Some((gap_start, gap_span)) = self
                            .renderer
                            .first_gap_between(base, highest.saturating_add(1))
                        {
                            let distance = highest.saturating_sub(gap_start);
                            trace!(
                                target = "client::backfill",
                                base, highest, gap_start, gap_span, "detected history gap"
                            );
                            if gap_span > 0
                                && distance > BACKFILL_LOOKAHEAD_ROWS as u64
                                && self.last_gap_backfill_start != Some(gap_start)
                                && !self.is_range_pending(
                                    gap_start,
                                    gap_start.saturating_add(BACKFILL_MAX_ROWS_PER_REQUEST as u64),
                                )
                            {
                                if let Some(last) = self.last_backfill_request_at {
                                    if last.elapsed() < BACKFILL_MIN_INTERVAL {
                                        return Ok(());
                                    }
                                }
                                let count = gap_span.clamp(1, BACKFILL_MAX_ROWS_PER_REQUEST);
                                self.send_backfill_request(subscription, gap_start, count)?;
                                self.last_backfill_request_at = Some(Instant::now());
                                self.last_gap_backfill_start = Some(gap_start);
                                return Ok(());
                            }
                        }
                    }
                }
            }

            if !self.renderer.is_following_tail() {
                if let Some(highest) = self.highest_loaded_row {
                    if let Some(base) = self.known_base_row {
                        if highest.saturating_sub(base) <= BACKFILL_LOOKAHEAD_ROWS as u64 {
                            // Not far enough from base to justify tail backfill
                            // continue to general gap handling below
                        } else {
                            let mut tail_start =
                                highest.saturating_sub(BACKFILL_LOOKAHEAD_ROWS as u64);
                            if let Some(base) = self.known_base_row {
                                tail_start = tail_start.max(base);
                            }
                            if self.last_tail_backfill_start != Some(tail_start)
                                && !self.is_range_pending(
                                    tail_start,
                                    tail_start.saturating_add(BACKFILL_MAX_ROWS_PER_REQUEST as u64),
                                )
                            {
                                if let Some(last) = self.last_backfill_request_at {
                                    if last.elapsed() < BACKFILL_MIN_INTERVAL {
                                        return Ok(());
                                    }
                                }
                                self.send_backfill_request(
                                    subscription,
                                    tail_start,
                                    BACKFILL_MAX_ROWS_PER_REQUEST,
                                )?;
                                self.last_tail_backfill_start = Some(tail_start);
                                self.last_backfill_request_at = Some(Instant::now());
                                return Ok(());
                            }
                            // fall through if tail request already pending
                        }
                    } else {
                        let tail_start = highest.saturating_sub(BACKFILL_LOOKAHEAD_ROWS as u64);
                        if self.last_tail_backfill_start != Some(tail_start)
                            && !self.is_range_pending(
                                tail_start,
                                tail_start.saturating_add(BACKFILL_MAX_ROWS_PER_REQUEST as u64),
                            )
                        {
                            if let Some(last) = self.last_backfill_request_at {
                                if last.elapsed() < BACKFILL_MIN_INTERVAL {
                                    return Ok(());
                                }
                            }
                            self.send_backfill_request(
                                subscription,
                                tail_start,
                                BACKFILL_MAX_ROWS_PER_REQUEST,
                            )?;
                            self.last_tail_backfill_start = Some(tail_start);
                            self.last_backfill_request_at = Some(Instant::now());
                            return Ok(());
                        }
                    }
                }
            }
        }

        if self.pending_backfills.len() >= BACKFILL_MAX_PENDING_REQUESTS {
            return Ok(());
        }
        if let Some(last) = self.last_backfill_request_at {
            if last.elapsed() < BACKFILL_MIN_INTERVAL {
                return Ok(());
            }
        }
        let next_range = self.renderer.first_unloaded_range(BACKFILL_LOOKAHEAD_ROWS);
        if self.renderer.is_following_tail() && self.pending_backfills.is_empty() {
            let has_gap = match (self.known_base_row, self.highest_loaded_row) {
                (Some(base), Some(highest)) => {
                    base < highest
                        && self
                            .renderer
                            .first_gap_between(base, highest.saturating_add(1))
                            .is_some()
                }
                (None, Some(_)) => true,
                _ => false,
            };
            let has_unloaded = next_range.is_some();
            if !has_gap && !has_unloaded {
                trace!(
                    target = "client::backfill",
                    base = ?self.known_base_row,
                    highest = ?self.highest_loaded_row,
                    "skip due to follow tail"
                );
                return Ok(());
            }
        }
        let Some((start, span)) = next_range else {
            return Ok(());
        };
        if span == 0 {
            return Ok(());
        }
        if self.renderer.is_following_tail() && self.pending_backfills.is_empty() {
            if let Some(highest) = self.highest_loaded_row {
                if start > highest.saturating_add(1) && self.next_backfill_request_id > 1 {
                    trace!(
                        target = "client::backfill",
                        start, highest, "skip pending rows beyond tail"
                    );
                    return Ok(());
                }
            }
        }
        if self.renderer.is_following_tail()
            && self.pending_backfills.is_empty()
            && let (Some(base), Some(highest)) = (self.known_base_row, self.highest_loaded_row)
            && base == 0
            && start > highest
            && self.next_backfill_request_id == 1
            && self.renderer.total_rows() <= BACKFILL_LOOKAHEAD_ROWS as u64
            && self
                .renderer
                .first_gap_between(base, highest.saturating_add(1))
                .is_none()
        {
            return Ok(());
        }
        let tail_hint = self
            .highest_loaded_row
            .map(|row| row.saturating_sub(BACKFILL_LOOKAHEAD_ROWS as u64));
        let mut request_start = start;
        let mut request_span = span;
        let mut matched_unretried_range = false;
        if let Some(range) = self.empty_tail_ranges.iter().find(|range| {
            !range.retry_attempted
                && Self::ranges_overlap(
                    start,
                    start.saturating_add(span as u64),
                    range.start,
                    range.end,
                )
        }) {
            request_start = range.start;
            request_span = range.end.saturating_sub(range.start) as u32;
            matched_unretried_range = true;
        }
        if !matched_unretried_range {
            request_start = match (self.known_base_row, tail_hint) {
                (Some(base), Some(tail)) => request_start.max(base).max(tail),
                (Some(base), None) => request_start.max(base),
                (None, Some(tail)) => request_start.max(tail),
                (None, None) => request_start,
            };
        }
        let capped = request_span.min(BACKFILL_MAX_ROWS_PER_REQUEST);
        if capped == 0 {
            return Ok(());
        }
        if let (Some(base), Some(highest)) = (self.known_base_row, self.highest_loaded_row) {
            if base < highest && request_start > base {
                if let Some((gap_start, _)) = self.renderer.first_gap_between(base, request_start) {
                    if gap_start < request_start
                        && highest.saturating_sub(base) > BACKFILL_LOOKAHEAD_ROWS as u64
                        && highest.saturating_sub(gap_start) > BACKFILL_LOOKAHEAD_ROWS as u64
                    {
                        request_start = gap_start;
                    }
                }
            }
        }
        if let Some(highest) = self.highest_loaded_row {
            let max_start = highest.saturating_add(1);
            if request_start > max_start {
                request_start = max_start;
            }
        }
        let request_end = request_start.saturating_add(capped as u64);
        let overlaps_trimmed = self.empty_tail_ranges.iter().any(|range| {
            range.retry_attempted
                && Self::ranges_overlap(request_start, request_end, range.start, range.end)
        });
        if overlaps_trimmed {
            return Ok(());
        }
        if self.should_defer_empty_retry(request_start, request_end) {
            return Ok(());
        }
        if self.is_range_pending(request_start, request_end) {
            return Ok(());
        }
        self.send_backfill_request(subscription, request_start, capped)?;
        self.last_backfill_request_at = Some(Instant::now());
        if self
            .last_tail_backfill_start
            .is_some_and(|prev| prev == request_start)
        {
            // keep marker until response arrives
        } else {
            self.last_tail_backfill_start = None;
        }
        Ok(())
    }

    fn is_range_pending(&self, start: u64, end: u64) -> bool {
        self.pending_backfills
            .iter()
            .any(|req| Self::ranges_overlap(start, end, req.start, req.end))
    }

    fn record_empty_tail_range(&mut self, start: u64, end: u64, trimmed: bool) {
        if start >= end {
            return;
        }
        let highest = self.highest_loaded_row;
        if let Some(pos) = self
            .empty_tail_ranges
            .iter()
            .position(|range| Self::ranges_overlap(start, end, range.start, range.end))
        {
            let range = &mut self.empty_tail_ranges[pos];
            range.start = range.start.min(start);
            range.end = range.end.max(end);
            range.recorded_at = Instant::now();
            range.highest_at = highest;
            if trimmed {
                range.retry_attempted = true;
            }
        } else {
            self.empty_tail_ranges.push(EmptyTailRange {
                start,
                end,
                recorded_at: Instant::now(),
                highest_at: highest,
                retry_attempted: trimmed,
            });
        }
        let known_base = self.known_base_row.unwrap_or(start);
        if end > known_base {
            self.known_base_row = Some(end);
        }
    }

    fn clear_empty_tail_ranges(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        self.empty_tail_ranges
            .retain(|range| !Self::ranges_overlap(start, end, range.start, range.end));
    }

    fn should_defer_empty_retry(&mut self, start: u64, end: u64) -> bool {
        if start >= end {
            return false;
        }
        if let Some(range) = self
            .empty_tail_ranges
            .iter_mut()
            .find(|range| Self::ranges_overlap(start, end, range.start, range.end))
        {
            let now = Instant::now();
            if !range.retry_attempted {
                range.retry_attempted = true;
                range.recorded_at = now;
                return false;
            }
            if range.highest_at != self.highest_loaded_row {
                range.recorded_at = now;
                return false;
            }
            let elapsed = now.duration_since(range.recorded_at);
            if elapsed >= BACKFILL_MIN_INTERVAL {
                range.recorded_at = now;
                return false;
            }
            trace!(
                target = "client::backfill",
                start,
                end,
                elapsed_ms = elapsed.as_millis() as u64,
                "deferring retry for empty tail range"
            );
            true
        } else {
            false
        }
    }

    fn send_backfill_request(
        &mut self,
        subscription: u64,
        start: u64,
        count: u32,
    ) -> Result<(), ClientError> {
        if count == 0 {
            return Ok(());
        }
        let request_id = self.next_backfill_request_id;
        self.next_backfill_request_id = self.next_backfill_request_id.saturating_add(1);
        let frame = WireClientFrame::RequestBackfill {
            subscription,
            request_id,
            start_row: start,
            count,
        };
        let bytes = protocol::encode_client_frame_binary(&frame);
        self.transport
            .send_bytes(&bytes)
            .map_err(ClientError::Transport)?;
        let end = start.saturating_add(count as u64);
        self.pending_backfills.push(BackfillRequestState {
            id: request_id,
            start,
            end,
            issued_at: Instant::now(),
            more_expected: false,
        });
        trace!(
            target = "client::backfill",
            request_id, subscription, start, count, "requesting history backfill"
        );
        Ok(())
    }

    fn handle_history_backfill(
        &mut self,
        _subscription: u64,
        request_id: u64,
        start_row: u64,
        count: u32,
        updates: Vec<WireUpdate>,
        more: bool,
    ) -> Result<(), ClientError> {
        trace!(
            target = "client::backfill",
            request_id,
            start_row,
            count,
            updates = updates.len(),
            more,
            "received history backfill"
        );
        if tracing::enabled!(Level::TRACE) {
            let mut preview: Vec<String> = Vec::new();
            for update in updates.iter().take(3) {
                if let WireUpdate::Row { row, cells, .. } = update {
                    let text: String = cells.iter().map(|cell| decode_wire_cell(*cell).0).collect();
                    preview.push(format!("{row}={:?}", text.trim_end_matches(' ')));
                }
            }
            if !preview.is_empty() {
                trace!(target = "client::backfill", request_id, sample = ?preview);
            }
        }
        let mut touched_rows: Vec<u64> = Vec::new();
        let mut observed_trim = false;
        for update in &updates {
            match update {
                WireUpdate::Cell { row, .. }
                | WireUpdate::Row { row, .. }
                | WireUpdate::RowSegment { row, .. } => {
                    touched_rows.push(*row as u64);
                }
                WireUpdate::Rect { rows, .. } => {
                    let start = rows[0] as u64;
                    let end = rows[1] as u64;
                    for r in start..end {
                        touched_rows.push(r);
                    }
                }
                WireUpdate::Trim { .. } => {
                    observed_trim = true;
                }
                WireUpdate::Style { .. } => {}
            }
            let (update_kind, row_hint, seq_hint) = Self::update_debug_metadata(update);
            self.renderer.set_debug_update_context(
                "history_backfill",
                update_kind,
                row_hint,
                seq_hint,
            );
            self.apply_wire_update(update);
            self.renderer.clear_debug_update_context();
        }
        touched_rows.sort_unstable();
        touched_rows.dedup();

        if let Some(pos) = self
            .pending_backfills
            .iter_mut()
            .position(|req| req.id == request_id)
        {
            if more {
                let state = &mut self.pending_backfills[pos];
                state.issued_at = Instant::now();
                state.start = start_row;
                state.end = start_row.saturating_add(count as u64);
                state.more_expected = true;
            } else {
                self.pending_backfills.remove(pos);
            }
        }

        if !more {
            let end = start_row.saturating_add(count as u64);
            self.finalize_backfill_range(start_row, end, &touched_rows);
            let trimmed = observed_trim || self.last_backfill_trimmed;
            if updates.is_empty() {
                self.record_empty_tail_range(start_row, end, trimmed);
            } else {
                self.clear_empty_tail_ranges(start_row, end);
                self.last_backfill_trimmed = false;
            }
            self.last_backfill_trimmed = trimmed;
            self.last_backfill_request_at = None;
            if self
                .last_tail_backfill_start
                .is_some_and(|prev| prev == start_row)
            {
                self.last_tail_backfill_start = None;
            }
            if self
                .last_gap_backfill_start
                .is_some_and(|prev| prev == start_row)
            {
                self.last_gap_backfill_start = None;
            }
        }

        self.force_render = true;
        Ok(())
    }

    fn finalize_backfill_range(&mut self, start: u64, end: u64, touched_rows: &[u64]) {
        if start >= end {
            return;
        }
        if touched_rows.is_empty() {
            let trimmed_floor = self.renderer.base_row();
            let clamp_start = start.max(trimmed_floor);
            if clamp_start >= end {
                return;
            }
            for row in clamp_start..end {
                self.renderer.mark_row_missing(row);
            }
            return;
        }
        let mut bounds_start = start;
        let mut bounds_end = end;
        if let Some(&first) = touched_rows.first() {
            bounds_start = bounds_start.min(first);
        }
        if let Some(&last) = touched_rows.last() {
            bounds_end = bounds_end.max(last.saturating_add(1));
        }
        if let Some(base) = self.known_base_row {
            bounds_start = bounds_start.max(base);
        }
        for row in start..end {
            if touched_rows.binary_search(&row).is_err() {
                self.renderer.mark_row_missing(row);
            }
        }
        if bounds_start <= self.renderer.base_row() && bounds_start < bounds_end {
            self.renderer.set_base_row(bounds_start);
        }
    }

    fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
        a_start < b_end && b_start < a_end
    }

    fn update_debug_metadata(update: &WireUpdate) -> (&'static str, Option<u64>, Option<Seq>) {
        match update {
            WireUpdate::Cell { row, seq, .. } => ("cell", Some(*row as u64), Some(*seq)),
            WireUpdate::Row { row, seq, .. } => ("row", Some(*row as u64), Some(*seq)),
            WireUpdate::RowSegment { row, seq, .. } => {
                ("row_segment", Some(*row as u64), Some(*seq))
            }
            WireUpdate::Rect { rows, seq, .. } => ("rect", Some(rows[0] as u64), Some(*seq)),
            WireUpdate::Trim { start, .. } => ("trim", Some(*start as u64), None),
            WireUpdate::Style { seq, .. } => ("style", None, Some(*seq)),
        }
    }

    fn apply_wire_update(&mut self, update: &WireUpdate) {
        use CursorHint::*;

        let mut cursor_hint: Option<CursorHint> = None;

        match update {
            WireUpdate::Cell {
                row,
                col,
                seq,
                cell,
            } => {
                trace!(
                    target = "client::render",
                    kind = "cell",
                    row = *row,
                    col = *col,
                    seq = *seq
                );
                let (ch, style) = decode_wire_cell(*cell);
                self.renderer
                    .apply_cell(*row as usize, *col as usize, *seq, ch, style);
                self.note_loaded_row(*row as u64);
                self.clear_empty_tail_ranges(*row as u64, (*row as u64).saturating_add(1));
                if !self.cursor_authoritative && !self.cursor_authoritative_pending {
                    let width = self.renderer.effective_row_width(*row as u64);
                    let target = width.max((*col as usize).saturating_add(1));
                    cursor_hint = Some(Exact(*row as usize, target));
                }
            }
            WireUpdate::Row { row, seq, cells } => {
                trace!(
                    target = "client::render",
                    kind = "row",
                    row = *row,
                    seq = *seq,
                    cols = cells.len()
                );
                let decoded: Vec<(char, Option<u32>)> =
                    cells.iter().map(|cell| decode_wire_cell(*cell)).collect();
                self.renderer
                    .apply_row_from_cells(*row as usize, *seq, &decoded);
                self.note_loaded_row(*row as u64);
                self.clear_empty_tail_ranges(*row as u64, (*row as u64).saturating_add(1));
                if !self.cursor_authoritative && !self.cursor_authoritative_pending {
                    let width = self.renderer.effective_row_width(*row as u64);
                    cursor_hint = Some(Exact(*row as usize, width));
                }
            }
            WireUpdate::Rect {
                rows,
                cols,
                seq,
                cell,
            } => {
                trace!(
                    target = "client::render",
                    kind = "rect",
                    rows = ?rows,
                    cols = ?cols,
                    seq = *seq
                );
                let row_range = rows[0] as usize..rows[1] as usize;
                let col_range = cols[0] as usize..cols[1] as usize;
                let (ch, style) = decode_wire_cell(*cell);
                self.renderer
                    .apply_rect(row_range, col_range, *seq, ch, style);
                for r in rows[0]..rows[1] {
                    self.note_loaded_row(r as u64);
                }
                self.clear_empty_tail_ranges(rows[0] as u64, rows[1] as u64);
                if !self.cursor_authoritative && !self.cursor_authoritative_pending {
                    if let Some(&target_row) = rows.last() {
                        let row_idx = target_row.saturating_sub(1) as usize;
                        let width = self.renderer.effective_row_width(row_idx as u64);
                        cursor_hint = Some(Exact(row_idx, width));
                    }
                }
            }
            WireUpdate::RowSegment {
                row,
                start_col,
                seq,
                cells,
            } => {
                if !cells.is_empty() {
                    trace!(
                        target = "client::render",
                        kind = "segment",
                        row = *row,
                        start_col = *start_col,
                        len = cells.len(),
                        seq = *seq
                    );
                    let mut segment = Vec::with_capacity(cells.len());
                    for (idx, cell) in cells.iter().enumerate() {
                        let (ch, style) = decode_wire_cell(*cell);
                        let col = *start_col as usize + idx;
                        segment.push((col, *seq, ch, style));
                    }
                    self.renderer.apply_segment(*row as usize, &segment);
                    self.note_loaded_row(*row as u64);
                    self.clear_empty_tail_ranges(*row as u64, (*row as u64).saturating_add(1));
                    if !self.cursor_authoritative && !self.cursor_authoritative_pending {
                        let width = self.renderer.effective_row_width(*row as u64);
                        cursor_hint = Some(Exact(*row as usize, width));
                    }
                }
            }
            WireUpdate::Trim { start, count, .. } => {
                trace!(
                    target = "client::render",
                    start = *start,
                    count = *count,
                    kind = "trim"
                );
                let start = *start as usize;
                let count = *count as usize;
                let trimmed_origin = (start as u64).saturating_add(count as u64);
                self.renderer.apply_trim(start, count);
                let update_base = match self.known_base_row {
                    Some(base) => base < trimmed_origin,
                    None => true,
                };
                if update_base {
                    self.known_base_row = Some(trimmed_origin);
                }
                if let Some(highest) = self.highest_loaded_row {
                    let trimmed_end = (start + count) as u64;
                    if highest < trimmed_end {
                        self.highest_loaded_row = None;
                    }
                }
                self.pending_predictions.values_mut().for_each(|pending| {
                    pending.positions.retain(|pos| pos.row >= start + count);
                });
                self.pending_predictions
                    .retain(|_, pending| !pending.positions.is_empty());
                if self.pending_predictions.is_empty() && self.renderer.has_active_predictions() {
                    self.renderer.clear_all_predictions();
                }
                self.update_prediction_overlay();
                if self.cursor_row >= start && self.cursor_row < start + count {
                    self.cursor_row = start + count;
                    self.cursor_col = 0;
                }
                self.force_render = true;
            }
            WireUpdate::Style {
                id,
                seq,
                fg,
                bg,
                attrs,
            } => {
                self.renderer.set_style(*id, *fg, *bg, *attrs);
                self.last_seq = cmp::max(self.last_seq, *seq);
            }
        }

        if matches!(
            update,
            WireUpdate::Cell { .. }
                | WireUpdate::Row { .. }
                | WireUpdate::Rect { .. }
                | WireUpdate::RowSegment { .. }
        ) {
            self.has_loaded_rows = true;
        }

        if let Some(hint) = cursor_hint {
            let previous_row = self.cursor_row;
            let previous_col = self.cursor_col;
            match hint {
                Exact(row, col) => {
                    let mut target_col = col;
                    let committed = self.renderer.committed_row_width(row as u64);
                    target_col = target_col.min(committed);
                    if self.row_has_predictions(row) {
                        let predicted_width = self.renderer.predicted_row_width(row as u64);
                        target_col =
                            target_col.max(predicted_width.min(committed.max(predicted_width)));
                        if row == previous_row {
                            target_col = target_col.max(previous_col);
                        }
                    } else {
                        target_col = target_col.min(committed);
                    }
                    self.cursor_row = row;
                    self.cursor_col = target_col;
                }
            }
        }

        self.sync_renderer_cursor();
    }

    fn apply_wire_cursor(&mut self, frame: &CursorFrame) {
        if !self.cursor_support {
            return;
        }

        let previous_row = self.cursor_row;
        let previous_col = self.cursor_col;
        self.cursor_seq = frame.seq;
        let new_row = frame.row as usize;
        let mut target_col = frame.col as usize;
        let total_cols = self.renderer.total_cols();
        if total_cols > 0 {
            target_col = target_col.min(total_cols);
        }

        let predicted_width = self.renderer.predicted_row_width(new_row as u64);
        let moving_left = new_row == previous_row && target_col < previous_col;
        if predicted_width > target_col || moving_left {
            self.discard_predictions_from_column(new_row, target_col);
        }

        let adjusted_predicted = self.renderer.predicted_row_width(new_row as u64);
        if adjusted_predicted > target_col {
            target_col = adjusted_predicted;
        }

        self.cursor_row = new_row;
        self.cursor_col = target_col;
        self.cursor_visible = frame.visible;
        self.cursor_authoritative = true;
        self.cursor_authoritative_pending = false;
        self.force_render = true;
        self.sync_renderer_cursor();
    }

    fn maybe_render(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            return Ok(());
        }

        if self.pending_render {
            let ready = self
                .last_render_at
                .map(|last| last.elapsed() >= self.render_interval)
                .unwrap_or(true);
            if ready {
                self.pending_render = false;
                self.force_render = false;
                self.render()?;
                self.last_render_at = Some(Instant::now());
            }
            return Ok(());
        }

        let dirty = self.renderer.take_dirty();
        if self.force_render || dirty {
            let now = Instant::now();
            if !self.force_render {
                if let Some(last) = self.last_render_at {
                    if now.duration_since(last) < self.render_interval {
                        self.pending_render = true;
                        if dirty {
                            self.renderer.mark_dirty();
                        }
                        return Ok(());
                    }
                }
            }
            self.force_render = false;
            self.render()?;
            self.last_render_at = Some(now);
        }
        Ok(())
    }

    fn render(&mut self) -> Result<(), ClientError> {
        if let Some(tui) = &mut self.tui {
            let _guard = PerfGuard::new("client_render_tui");
            let renderer = &mut self.renderer;
            tui.draw(|frame| renderer.render_frame(frame))
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        } else {
            let _guard = PerfGuard::new("client_render_simple");
            let mut stdout = io::stdout();
            execute!(stdout, MoveTo(0, 0), Clear(ClearType::All), Show)
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
            for line in self.renderer.visible_lines() {
                writeln!(stdout, "{line}").map_err(|err| {
                    ClientError::Transport(TransportError::Setup(err.to_string()))
                })?;
            }
            if let Some((col, row)) = self.renderer.cursor_viewport_position() {
                execute!(stdout, MoveTo(col, row), Show).map_err(|err| {
                    ClientError::Transport(TransportError::Setup(err.to_string()))
                })?;
            }
            stdout
                .flush()
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        }
        Ok(())
    }

    fn pump_input(&mut self) -> Result<(), ClientError> {
        let mut pending: Vec<Vec<u8>> = Vec::new();
        let mut disconnected = false;

        if let Some(rx) = &self.input_rx {
            loop {
                match rx.try_recv() {
                    Ok(bytes) => pending.push(bytes),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        } else if self.render_enabled {
            while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        if self.handle_super_shortcuts(&key)? {
                            continue;
                        }
                        if self.handle_control_shortcuts(&key)? {
                            continue;
                        }
                        if self.process_copy_mode_key(&key) {
                            continue;
                        }
                        if self.handle_local_key(&key) {
                            continue;
                        }
                        if let Some(bytes) = encode_key_event(key) {
                            pending.push(bytes);
                        }
                    }
                    Ok(Event::Paste(data)) => {
                        pending.push(data.into_bytes());
                    }
                    Ok(Event::Resize(cols, rows)) => {
                        self.renderer.on_resize(cols, rows);
                        self.force_render = true;
                        self.send_resize(cols, rows)?;
                    }
                    Ok(Event::Mouse(mouse)) => {
                        if self.handle_mouse_event(&mouse)? {
                            continue;
                        }
                        if self.forward_mouse_to_host {
                            if let Some(encoded) = encode_mouse_event(&mouse) {
                                pending.push(encoded);
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("⚠️  input read error: {err}");
                        break;
                    }
                    _ => {}
                }
            }
        }

        if disconnected {
            self.input_rx = None;
        }

        for bytes in pending {
            self.send_input(&bytes)?;
        }
        Ok(())
    }

    fn process_copy_mode_key(&mut self, key: &KeyEvent) -> bool {
        if self.handle_tmux_prefix(key) {
            return true;
        }

        if self.copy_mode.is_some() {
            if self.consume_copy_mode_pending_input(key) {
                return true;
            }
            if is_copy_shortcut(key) {
                self.copy_selection_to_clipboard(true);
                return true;
            }
            let (mode, selection_active) = {
                let state = self.copy_mode.as_ref().unwrap();
                (state.mode, state.selection_active)
            };
            if let Some(command) = copy_mode_command_for_key(mode, selection_active, key) {
                self.execute_copy_mode_command(command);
                return true;
            }
            return false;
        }

        if matches!(key.code, KeyCode::PageUp) {
            self.enter_copy_mode();
            if self.copy_mode.is_some() {
                self.execute_copy_mode_command(CopyModeCommand::Page { delta: -1 });
            }
            return true;
        }

        if key.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c) = key.code {
                if c.eq_ignore_ascii_case(&'[') {
                    self.enter_copy_mode();
                    return true;
                }
            }
        }

        false
    }

    fn handle_tmux_prefix(&mut self, key: &KeyEvent) -> bool {
        self.expire_tmux_prefix();

        let is_ctrl_b = matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'b'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SUPER);

        if is_ctrl_b {
            self.tmux_prefix_started_at = Some(Instant::now());
            return true;
        }

        if self.tmux_prefix_started_at.take().is_some() {
            match key.code {
                KeyCode::Char('[') => {
                    self.enter_copy_mode();
                    return true;
                }
                KeyCode::PageUp => {
                    self.enter_copy_mode();
                    if self.copy_mode.is_some() {
                        self.execute_copy_mode_command(CopyModeCommand::Page { delta: -1 });
                    }
                    return true;
                }
                KeyCode::PageDown => {
                    self.enter_copy_mode();
                    if self.copy_mode.is_some() {
                        self.execute_copy_mode_command(CopyModeCommand::Page { delta: 1 });
                    }
                    return true;
                }
                KeyCode::Char(']') => {
                    self.paste_from_clipboard();
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    fn expire_tmux_prefix(&mut self) {
        if let Some(started) = self.tmux_prefix_started_at {
            if started.elapsed() >= TMUX_PREFIX_TIMEOUT {
                self.tmux_prefix_started_at = None;
            }
        }
    }

    fn consume_copy_mode_pending_input(&mut self, key: &KeyEvent) -> bool {
        let pending = match self.copy_mode.as_mut() {
            Some(state) => match state.pending_input.as_mut() {
                Some(pending) => pending,
                None => return false,
            },
            None => return false,
        };

        match pending {
            CopyModePendingInput::Search { direction, buffer } => match key.code {
                KeyCode::Esc => {
                    if let Some(state) = self.copy_mode.as_mut() {
                        state.pending_input = None;
                    }
                    self.update_copy_mode_status();
                    self.force_render = true;
                    true
                }
                KeyCode::Enter => {
                    let pattern = buffer.clone();
                    let direction = *direction;
                    if let Some(state) = self.copy_mode.as_mut() {
                        state.pending_input = None;
                    }
                    let found = self.perform_copy_mode_search(direction, &pattern);
                    if !found {
                        self.renderer.set_status_message(Some(format!(
                            r#"copy-mode: "{pattern}" not found"#
                        )));
                    }
                    self.update_copy_mode_status();
                    self.force_render = true;
                    true
                }
                KeyCode::Backspace => {
                    buffer.pop();
                    self.update_copy_mode_prompt();
                    self.force_render = true;
                    true
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    buffer.push(c);
                    self.update_copy_mode_prompt();
                    self.force_render = true;
                    true
                }
                _ => false,
            },
        }
    }

    fn update_copy_mode_prompt(&mut self) {
        if let Some(state) = &self.copy_mode {
            if let Some(CopyModePendingInput::Search { direction, buffer }) = &state.pending_input {
                let prefix = match direction {
                    CopyModeSearchDirection::Forward => '/',
                    CopyModeSearchDirection::Backward => '?',
                };
                let text = format!("{prefix}{buffer}");
                self.renderer.set_status_message(Some(text));
            }
        }
    }

    fn execute_copy_mode_command(&mut self, command: CopyModeCommand) {
        match command {
            CopyModeCommand::Move { rows, cols } => self.move_copy_cursor(rows, cols),
            CopyModeCommand::MoveToLineStart => self.move_copy_cursor_line_start(),
            CopyModeCommand::MoveToLineEnd => self.move_copy_cursor_line_end(),
            CopyModeCommand::Page { delta } => self.move_copy_cursor_page(delta),
            CopyModeCommand::HalfPage { delta } => self.move_copy_cursor_half_page(delta),
            CopyModeCommand::JumpTop => self.jump_copy_cursor_to_top(),
            CopyModeCommand::JumpBottom => self.jump_copy_cursor_to_bottom(),
            CopyModeCommand::MoveWord(motion) => self.move_copy_cursor_word(motion),
            CopyModeCommand::BeginSelection => {
                if let Some(state) = self.copy_mode.as_mut() {
                    state.begin_selection(SelectionMode::Character);
                }
                self.renderer.set_follow_tail(false);
                self.update_copy_mode_selection();
                self.update_copy_mode_status();
            }
            CopyModeCommand::ClearSelection => {
                if let Some(state) = self.copy_mode.as_mut() {
                    state.clear_selection();
                    state.selection_mode = SelectionMode::Character;
                }
                self.update_copy_mode_selection();
                self.update_copy_mode_status();
            }
            CopyModeCommand::ToggleSelection => {
                if let Some(state) = self.copy_mode.as_mut() {
                    state.toggle_selection();
                    if state.selection_active {
                        self.renderer.set_follow_tail(false);
                    }
                }
                self.update_copy_mode_selection();
                self.update_copy_mode_status();
            }
            CopyModeCommand::SetSelectionMode(selection_mode) => {
                if let Some(state) = self.copy_mode.as_mut() {
                    if state.selection_active {
                        if state.selection_mode == selection_mode {
                            state.selection_mode = SelectionMode::Character;
                        } else {
                            state.selection_mode = selection_mode;
                        }
                    } else {
                        state.selection_mode = selection_mode;
                        state.selection_active = true;
                        state.anchor = state.cursor;
                    }
                }
                self.update_copy_mode_selection();
                self.renderer.set_follow_tail(false);
                self.update_copy_mode_status();
            }
            CopyModeCommand::CopySelection => self.copy_selection_to_clipboard(false),
            CopyModeCommand::CopySelectionAndExit => self.copy_selection_to_clipboard(true),
            CopyModeCommand::Cancel => self.exit_copy_mode(),
            CopyModeCommand::SetMode(mode) => {
                if let Some(state) = self.copy_mode.as_mut() {
                    state.mode = mode;
                }
                self.update_copy_mode_status();
            }
            CopyModeCommand::Search(direction) => {
                self.start_copy_mode_search(direction);
            }
            CopyModeCommand::RepeatLastSearch(direction_hint) => {
                self.repeat_last_search(direction_hint);
            }
        }
    }

    fn start_copy_mode_search(&mut self, direction: CopyModeSearchDirection) {
        if let Some(state) = self.copy_mode.as_mut() {
            state.pending_input = Some(CopyModePendingInput::Search {
                direction,
                buffer: String::new(),
            });
        }
        self.update_copy_mode_prompt();
        self.force_render = true;
    }

    fn repeat_last_search(&mut self, direction_hint: CopyModeSearchDirection) {
        let (pattern, base_direction) = match self.copy_mode.as_ref() {
            Some(state) => match &state.last_search {
                Some(search) => (search.pattern.clone(), search.direction),
                None => return,
            },
            None => return,
        };

        let direction = match direction_hint {
            CopyModeSearchDirection::Forward => base_direction,
            CopyModeSearchDirection::Backward => reverse_search_direction(base_direction),
        };

        if !self.perform_copy_mode_search(direction, &pattern) {
            self.renderer
                .set_status_message(Some(format!(r#"copy-mode: "{pattern}" not found"#)));
            self.force_render = true;
        }
    }

    fn perform_copy_mode_search(
        &mut self,
        direction: CopyModeSearchDirection,
        pattern: &str,
    ) -> bool {
        if pattern.is_empty() {
            return false;
        }
        let (start_row, start_col) = match self.copy_mode.as_ref() {
            Some(state) => (state.cursor.row, state.cursor.col),
            None => return false,
        };
        let total_rows = self.renderer.total_rows();
        if total_rows == 0 {
            return false;
        }

        let mut found: Option<SelectionPosition> = None;
        match direction {
            CopyModeSearchDirection::Forward => {
                let mut row = start_row;
                let last_row = total_rows.saturating_sub(1);
                let mut first = true;
                while row <= last_row {
                    if let Some(text) = self.renderer.row_text(row) {
                        let start_idx = if first {
                            start_col.saturating_add(1)
                        } else {
                            0
                        };
                        if start_idx < text.len() {
                            if let Some(offset) = text[start_idx..].find(pattern) {
                                let col = start_idx + offset;
                                found = Some(SelectionPosition { row, col });
                                break;
                            }
                        }
                    }
                    if row == last_row {
                        break;
                    }
                    row = row.saturating_add(1);
                    first = false;
                }
            }
            CopyModeSearchDirection::Backward => {
                let mut row = start_row;
                let lower_bound = self.renderer.base_row();
                let mut first = true;
                loop {
                    if let Some(text) = self.renderer.row_text(row) {
                        let end_idx = if first { start_col } else { text.len() };
                        if end_idx > 0 && end_idx <= text.len() {
                            if let Some(offset) = text[..end_idx].rfind(pattern) {
                                found = Some(SelectionPosition { row, col: offset });
                                break;
                            }
                        }
                    }
                    if row <= lower_bound {
                        break;
                    }
                    if row == 0 {
                        break;
                    }
                    row -= 1;
                    first = false;
                }
            }
        }

        let pattern_owned = pattern.to_string();
        if let Some(position) = found {
            if let Some(state) = self.copy_mode.as_mut() {
                state.selection_active = false;
                state.last_search = Some(CopyModeSearch {
                    direction,
                    pattern: pattern_owned.clone(),
                });
            }
            self.set_copy_cursor_position(position, true);
            self.update_copy_mode_status();
            true
        } else {
            if let Some(state) = self.copy_mode.as_mut() {
                state.last_search = Some(CopyModeSearch {
                    direction,
                    pattern: pattern_owned,
                });
            }
            false
        }
    }

    fn set_copy_cursor_position(&mut self, position: SelectionPosition, ensure_visible: bool) {
        let (selection_active, anchor, mode) = match self.copy_mode.as_mut() {
            Some(state) => {
                state.cursor = position;
                if !state.selection_active {
                    state.anchor = position;
                }
                (state.selection_active, state.anchor, state.selection_mode)
            }
            None => return,
        };

        if selection_active {
            self.renderer.set_selection(anchor, position, mode);
        } else {
            self.renderer.clear_selection();
        }
        self.renderer.set_follow_tail(false);
        if ensure_visible {
            self.renderer.ensure_row_visible(position.row);
        }
        self.force_render = true;
    }

    fn update_copy_mode_selection(&mut self) {
        if let Some(state) = &self.copy_mode {
            if state.selection_active {
                self.renderer
                    .set_selection(state.anchor, state.cursor, state.selection_mode);
            } else {
                self.renderer.clear_selection();
            }
            self.force_render = true;
        }
    }

    fn update_copy_mode_status(&mut self) {
        let Some(state) = &self.copy_mode else {
            self.renderer.set_status_message::<String>(None);
            return;
        };
        if state.pending_input.is_some() {
            self.update_copy_mode_prompt();
            return;
        }
        let mode_label = match state.mode {
            CopyModeKeySet::Vi => "vi",
            CopyModeKeySet::Emacs => "emacs",
        };
        let focus = if state.selection_active {
            match state.selection_mode {
                SelectionMode::Character => "select (char)",
                SelectionMode::Line => "select (line)",
                SelectionMode::Block => "select (block)",
            }
        } else {
            "cursor"
        };
        let last_search = state
            .last_search
            .as_ref()
            .map(|search| {
                let prefix = match search.direction {
                    CopyModeSearchDirection::Forward => '/',
                    CopyModeSearchDirection::Backward => '?',
                };
                format!(" • last {prefix}{}", search.pattern)
            })
            .unwrap_or_default();
        let selection_hint = if state.selection_active {
            "Space unmark"
        } else {
            "Space mark"
        };
        let text = format!(
            "copy-mode ({mode_label}) • {focus} • {selection_hint} • V line • Ctrl+V block • Cmd/Ctrl+Shift+C copy • n/N repeat • Esc/q exit{last_search}"
        );
        self.renderer.set_status_message(Some(text));
    }

    fn handle_mouse_event(&mut self, mouse: &MouseEvent) -> Result<bool, ClientError> {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.handle_mouse_scroll(-MOUSE_SCROLL_LINES);
                return Ok(true);
            }
            MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(MOUSE_SCROLL_LINES);
                return Ok(true);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.copy_mode.is_some() {
                    self.handle_mouse_primary_down(mouse);
                    return Ok(true);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.copy_mode.is_some() {
                    self.handle_mouse_primary_drag(mouse);
                    return Ok(true);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.copy_mode.is_some() {
                    self.handle_mouse_primary_up();
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_mouse_scroll(&mut self, delta: isize) {
        self.apply_scroll_delta(delta, true);
    }

    fn handle_mouse_primary_down(&mut self, mouse: &MouseEvent) {
        if let Some(position) = self.mouse_position_to_selection(mouse) {
            if let Some(state) = self.copy_mode.as_mut() {
                state.cursor = position;
                state.anchor = position;
                state.selection_active = true;
                state.selection_mode = SelectionMode::Character;
                self.renderer
                    .set_selection(position, position, SelectionMode::Character);
                self.renderer.set_follow_tail(false);
                self.renderer.ensure_row_visible(position.row);
                self.update_copy_mode_status();
                self.force_render = true;
            }
        }
    }

    fn handle_mouse_primary_drag(&mut self, mouse: &MouseEvent) {
        if let Some(position) = self.mouse_position_to_selection(mouse) {
            if let Some(state) = self.copy_mode.as_mut() {
                if !state.selection_active {
                    state.begin_selection(SelectionMode::Character);
                }
                state.cursor = position;
            }
            if let Some(state) = &self.copy_mode {
                self.renderer
                    .set_selection(state.anchor, position, state.selection_mode);
            } else {
                self.renderer
                    .set_selection(position, position, SelectionMode::Character);
            }
            self.renderer.ensure_row_visible(position.row);
            self.force_render = true;
        }
        self.maybe_auto_scroll_drag(mouse);
    }

    fn handle_mouse_primary_up(&mut self) {
        self.update_copy_mode_status();
    }

    fn maybe_auto_scroll_drag(&mut self, mouse: &MouseEvent) {
        if self.copy_mode.is_none() {
            return;
        }

        let viewport_height = self.renderer.viewport_height();
        if viewport_height == 0 {
            return;
        }

        let edge_row = mouse.row as usize;
        if edge_row == 0 {
            self.apply_scroll_delta(-1, false);
        } else if edge_row >= viewport_height.saturating_sub(1) {
            self.apply_scroll_delta(1, false);
        }
    }

    fn apply_scroll_delta(&mut self, delta: isize, auto_enter_copy: bool) -> i64 {
        if delta == 0 {
            return 0;
        }

        if auto_enter_copy && delta < 0 && self.copy_mode.is_none() {
            self.enter_copy_mode();
        }

        let before_top = self.renderer.viewport_top();
        self.renderer.scroll_lines(delta);
        let after_top = self.renderer.viewport_top();
        let reached_tail = self.renderer.is_following_tail();
        let actual_delta = if after_top >= before_top {
            (after_top - before_top) as i64
        } else {
            -((before_top - after_top) as i64)
        };

        if self.copy_mode.is_some() && actual_delta != 0 {
            let (row, col) = {
                let state = self.copy_mode.as_ref().unwrap();
                (state.cursor.row, state.cursor.col)
            };
            let target_row = row as i64 + actual_delta;
            let new_pos = self.renderer.clamp_position(target_row, col as isize);
            self.set_copy_cursor_position(new_pos, false);
        } else if actual_delta > 0 && reached_tail {
            self.renderer.scroll_to_tail();
        }

        if self.copy_mode.is_some() && actual_delta > 0 && reached_tail {
            self.exit_copy_mode();
        }

        if actual_delta != 0 {
            self.force_render = true;
        }

        actual_delta
    }

    fn paste_from_clipboard(&mut self) {
        match clipboard_get() {
            Ok(contents) => {
                if contents.is_empty() {
                    self.renderer.set_status_message(Some("clipboard empty"));
                    self.force_render = true;
                    return;
                }
                if let Err(err) = self.send_input(contents.as_bytes()) {
                    eprintln!("⚠️  failed to paste clipboard: {err}");
                    self.renderer
                        .set_status_message(Some("clipboard paste failed"));
                    self.force_render = true;
                }
            }
            Err(err) => {
                eprintln!("⚠️  clipboard unavailable: {err}");
                self.renderer
                    .set_status_message(Some("clipboard unavailable"));
                self.force_render = true;
            }
        }
    }

    fn mouse_position_to_selection(&self, mouse: &MouseEvent) -> Option<SelectionPosition> {
        let viewport_height = self.renderer.viewport_height();
        if viewport_height == 0 {
            return None;
        }
        let row_offset = mouse.row as usize;
        if row_offset >= viewport_height {
            return None;
        }
        let absolute_row = self
            .renderer
            .viewport_top()
            .saturating_add(row_offset as u64);
        Some(
            self.renderer
                .clamp_position(absolute_row as i64, mouse.column as isize),
        )
    }

    fn enter_copy_mode(&mut self) {
        if self.copy_mode.is_some() {
            return;
        }
        let total_rows = self.renderer.total_rows();
        let total_cols = self.renderer.total_cols();
        if total_rows == 0 || total_cols == 0 {
            return;
        }
        let viewport_top = self.renderer.viewport_top();
        let viewport_height = self.renderer.viewport_height();
        let viewport_height_u64 = viewport_height as u64;
        let max_row = total_rows.saturating_sub(1);
        let start_row = viewport_top
            .saturating_add(viewport_height_u64.saturating_sub(1))
            .min(max_row);
        let start_pos = self.renderer.clamp_position(start_row as i64, 0);
        let mode = default_copy_mode_keyset();
        self.copy_mode = Some(CopyModeState::new(start_pos, mode));
        self.renderer.set_follow_tail(false);
        self.renderer.clear_selection();
        self.update_copy_mode_status();
        self.force_render = true;
    }

    fn exit_copy_mode(&mut self) {
        if self.copy_mode.take().is_some() {
            self.renderer.clear_selection();
            self.renderer.set_status_message::<String>(None);
            self.renderer.set_follow_tail(true);
            self.renderer.mark_dirty();
            self.force_render = true;
        }
    }

    fn move_copy_cursor(&mut self, delta_row: isize, delta_col: isize) {
        if self.copy_mode.is_none() {
            return;
        }
        let (row, col) = {
            let state = self.copy_mode.as_ref().unwrap();
            (state.cursor.row, state.cursor.col)
        };
        let target_row = row as i64 + delta_row as i64;
        let target_col = col as isize + delta_col;
        let new_pos = self.renderer.clamp_position(target_row, target_col);
        self.set_copy_cursor_position(new_pos, true);
    }

    fn move_copy_cursor_page(&mut self, pages: isize) {
        if pages == 0 || self.copy_mode.is_none() {
            return;
        }
        let step = self.renderer.viewport_height() as isize;
        if step == 0 {
            return;
        }
        let delta = pages.saturating_mul(step);
        let moved = self.apply_scroll_delta(delta, false);
        if moved == 0 {
            self.move_copy_cursor(delta, 0);
        }
    }

    fn move_copy_cursor_half_page(&mut self, delta: isize) {
        if delta == 0 || self.copy_mode.is_none() {
            return;
        }
        let height = self.renderer.viewport_height() as isize;
        if height == 0 {
            return;
        }
        let step = (height / 2).max(1);
        let lines = delta.saturating_mul(step);
        let moved = self.apply_scroll_delta(lines, false);
        if moved == 0 {
            self.move_copy_cursor(lines, 0);
        }
    }

    fn move_copy_cursor_word(&mut self, motion: WordMotion) {
        let Some(state) = self.copy_mode.as_ref() else {
            return;
        };
        let start = state.cursor;
        let target = match motion {
            WordMotion::NextStart => self.find_word_forward(start, ForwardWordKind::Start),
            WordMotion::NextEnd => self.find_word_forward(start, ForwardWordKind::End),
            WordMotion::PrevStart => self.find_word_backward(start),
        };
        if let Some(position) = target {
            self.set_copy_cursor_position(position, true);
        }
    }

    fn find_word_forward(
        &self,
        start: SelectionPosition,
        kind: ForwardWordKind,
    ) -> Option<SelectionPosition> {
        let total_rows = self.renderer.total_rows();
        if total_rows == 0 {
            return None;
        }
        let mut row = start.row;
        let mut first = true;
        while row < total_rows {
            let text = self.renderer.row_text(row).unwrap_or_default();
            let chars: Vec<char> = text.chars().collect();
            let result = match kind {
                ForwardWordKind::Start => {
                    find_next_word_start_in_line(&chars, if first { Some(start.col) } else { None })
                }
                ForwardWordKind::End => {
                    find_next_word_end_in_line(&chars, if first { Some(start.col) } else { None })
                }
            };
            if let Some(col) = result {
                return Some(SelectionPosition { row, col });
            }
            if row >= total_rows.saturating_sub(1) {
                break;
            }
            row = row.saturating_add(1);
            first = false;
        }
        None
    }

    fn find_word_backward(&self, start: SelectionPosition) -> Option<SelectionPosition> {
        let base_row = self.renderer.base_row();
        let mut row = start.row;
        let mut first = true;
        loop {
            if row < base_row {
                break;
            }
            let text = self.renderer.row_text(row).unwrap_or_default();
            let chars: Vec<char> = text.chars().collect();
            if let Some(col) =
                find_prev_word_start_in_line(&chars, if first { Some(start.col) } else { None })
            {
                return Some(SelectionPosition { row, col });
            }
            if row == base_row || row == 0 {
                break;
            }
            row -= 1;
            first = false;
        }
        None
    }

    fn move_copy_cursor_line_start(&mut self) {
        if self.copy_mode.is_none() {
            return;
        }
        let row = self.copy_mode.as_ref().unwrap().cursor.row;
        let new_pos = self.renderer.clamp_position(row as i64, 0);
        self.set_copy_cursor_position(new_pos, true);
    }

    fn move_copy_cursor_line_end(&mut self) {
        if self.copy_mode.is_none() {
            return;
        }
        let row = self.copy_mode.as_ref().unwrap().cursor.row;
        let row_width = self.renderer.row_display_width(row);
        let target_col = if row_width == 0 { 0 } else { row_width - 1 };
        let new_pos = self
            .renderer
            .clamp_position(row as i64, target_col as isize);
        self.set_copy_cursor_position(new_pos, true);
    }

    fn jump_copy_cursor_to_top(&mut self) {
        if self.copy_mode.is_none() {
            return;
        }
        let top = self.renderer.base_row();
        let position = self.renderer.clamp_position(top as i64, 0);
        self.set_copy_cursor_position(position, true);
    }

    fn jump_copy_cursor_to_bottom(&mut self) {
        if self.copy_mode.is_none() {
            return;
        }
        let last_row = self.renderer.total_rows().saturating_sub(1);
        let position = self.renderer.clamp_position(last_row as i64, 0);
        self.set_copy_cursor_position(position, true);
    }

    fn copy_selection_to_clipboard(&mut self, exit_after: bool) {
        let selection_active = self
            .copy_mode
            .as_ref()
            .map(|state| state.selection_active)
            .unwrap_or(false);
        if !selection_active {
            self.renderer
                .set_status_message(Some("copy-mode: no active selection"));
            self.force_render = true;
            if exit_after {
                self.exit_copy_mode();
            }
            return;
        }

        if let Some(text) = self.renderer.selection_text() {
            if let Err(err) = clipboard_set(&text) {
                eprintln!("⚠️  failed to copy selection: {err}");
            }
        }

        if exit_after {
            self.exit_copy_mode();
        } else {
            self.update_copy_mode_status();
        }

        self.force_render = true;
    }

    fn handle_control_shortcuts(&mut self, key: &KeyEvent) -> Result<bool, ClientError> {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'q') => {
                return Err(ClientError::Shutdown);
            }
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c') && self.copy_mode.is_some() => {
                self.exit_copy_mode();
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_super_shortcuts(&mut self, key: &KeyEvent) -> Result<bool, ClientError> {
        if !key.modifiers.contains(KeyModifiers::SUPER) {
            return Ok(false);
        }

        match key.code {
            KeyCode::Char(c) if c.eq_ignore_ascii_case(&'k') => {
                self.force_render = true;
                self.renderer.mark_dirty();
                self.request_viewport_clear()?;
                return Ok(true);
            }
            _ => {}
        }

        Ok(false)
    }

    fn handle_local_key(&mut self, key: &KeyEvent) -> bool {
        if !key.modifiers.intersects(KeyModifiers::ALT) {
            return false;
        }
        let lower = match key.code {
            KeyCode::Char(c) => c.to_ascii_lowercase(),
            _ => '\0',
        };
        match key.code {
            KeyCode::Up => {
                self.renderer.set_follow_tail(false);
                self.renderer.scroll_lines(-1);
                self.force_render = true;
                true
            }
            KeyCode::Down => {
                self.renderer.scroll_lines(1);
                self.force_render = true;
                true
            }
            KeyCode::PageUp => {
                self.renderer.set_follow_tail(false);
                self.renderer.scroll_pages(-1);
                self.force_render = true;
                true
            }
            KeyCode::PageDown => {
                self.renderer.scroll_pages(1);
                self.force_render = true;
                true
            }
            KeyCode::End => {
                self.renderer.scroll_to_tail();
                self.force_render = true;
                true
            }
            KeyCode::Home => {
                self.renderer.scroll_to_top();
                self.force_render = true;
                true
            }
            KeyCode::Char(_) if lower == 'f' => {
                self.renderer.toggle_follow_tail();
                self.force_render = true;
                true
            }
            KeyCode::Char(_) if lower == 'c' => {
                if let Some(state) = self.copy_mode.as_mut() {
                    state.clear_selection();
                    self.update_copy_mode_selection();
                    self.update_copy_mode_status();
                } else {
                    self.renderer.clear_selection();
                }
                self.force_render = true;
                true
            }
            KeyCode::Char(_) if lower == 'y' => {
                self.copy_selection_to_clipboard(true);
                true
            }
            _ => false,
        }
    }

    fn send_input(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        self.send_input_internal(bytes, true)
    }

    fn request_viewport_clear(&mut self) -> Result<(), ClientError> {
        self.send_viewport_command(ViewportCommand::Clear)?;
        const CLEAR: &[u8] = b"\x0c"; // Ctrl+L fallback for older hosts
        self.send_input_internal(CLEAR, false)
    }

    fn send_viewport_command(&mut self, command: ViewportCommand) -> Result<(), ClientError> {
        let frame = WireClientFrame::ViewportCommand { command };
        let encoded = protocol::encode_client_frame_binary(&frame);
        telemetry::record_bytes("client_input_frames", encoded.len());
        self.transport
            .send_bytes(&encoded)
            .map_err(ClientError::Transport)?;
        trace!(target = "client::outgoing", command = ?command, "viewport command sent");
        Ok(())
    }

    fn send_input_internal(
        &mut self,
        bytes: &[u8],
        allow_predictions: bool,
    ) -> Result<(), ClientError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.subscription_id.is_none() {
            trace!(
                target = "client::outgoing",
                "dropping input before handshake"
            );
            return Ok(());
        }
        self.input_seq = self.input_seq.saturating_add(1);
        telemetry::record_bytes("client_input_bytes", bytes.len());
        let frame = WireClientFrame::Input {
            seq: self.input_seq,
            data: bytes.to_vec(),
        };
        let encoded = protocol::encode_client_frame_binary(&frame);
        telemetry::record_bytes("client_input_frames", encoded.len());
        self.transport
            .send_bytes(&encoded)
            .map_err(ClientError::Transport)?;
        if tracing::enabled!(Level::TRACE) {
            trace!(
                target = "client::outgoing",
                seq = self.input_seq,
                bytes = bytes.len(),
                dump = %crate::telemetry::logging::hexdump(bytes),
                "input sent"
            );
        }
        if allow_predictions {
            self.register_prediction(self.input_seq, bytes);
        }
        Ok(())
    }

    fn set_authorization_state(&mut self, state: AuthorizationState, message: Option<String>) {
        if self.authorization_state == state && self.authorization_message == message {
            return;
        }
        self.authorization_state = state;
        self.authorization_hint_level = 0;
        match self.authorization_state {
            AuthorizationState::Waiting => {
                self.authorization_message =
                    Some(message.unwrap_or_else(|| AUTH_WAIT_MESSAGE.to_string()));
                if self.authorization_wait_started.is_none() {
                    self.authorization_wait_started = Some(Instant::now());
                }
                self.authorization_last_tick = Instant::now();
                self.authorization_spinner_index = 0;
                self.authorization_approved_since = None;
                self.authorization_shutdown_at = None;
            }
            AuthorizationState::Approved => {
                self.authorization_pending_hint = false;
                self.authorization_message =
                    Some(message.unwrap_or_else(|| AUTH_APPROVED_MESSAGE.to_string()));
                self.authorization_wait_started = None;
                self.authorization_last_tick = Instant::now();
                self.authorization_spinner_index = 0;
                self.authorization_approved_since = Some(Instant::now());
                self.authorization_shutdown_at = None;
            }
            AuthorizationState::Denied => {
                self.authorization_pending_hint = false;
                self.authorization_message =
                    Some(message.unwrap_or_else(|| AUTH_DENIED_MESSAGE.to_string()));
                self.authorization_wait_started = None;
                self.authorization_approved_since = None;
                self.authorization_shutdown_at = Some(Instant::now() + AUTH_DENIED_EXIT_DELAY);
            }
            AuthorizationState::Connecting => {
                self.authorization_pending_hint = false;
                self.authorization_message = message;
                self.authorization_wait_started = None;
                self.authorization_approved_since = None;
                self.authorization_shutdown_at = None;
                self.authorization_last_tick = Instant::now();
                self.authorization_spinner_index = 0;
            }
        }
        self.refresh_authorization_status();
    }

    fn refresh_authorization_status(&mut self) {
        match self.authorization_state {
            AuthorizationState::Waiting => {
                let base = self
                    .authorization_message
                    .clone()
                    .unwrap_or_else(|| AUTH_WAIT_MESSAGE.to_string());
                let spinner = AUTH_SPINNER_FRAMES
                    [self.authorization_spinner_index % AUTH_SPINNER_FRAMES.len()];
                let text = format!("{base} {spinner}");
                self.renderer.set_status_message(Some(text));
                self.force_render = true;
            }
            AuthorizationState::Denied => {
                if let Some(message) = &self.authorization_message {
                    self.renderer.set_status_message(Some(message.clone()));
                }
                self.force_render = true;
            }
            AuthorizationState::Approved => {
                if let Some(message) = &self.authorization_message {
                    self.renderer.set_status_message(Some(message.clone()));
                } else {
                    self.renderer.set_status_message::<String>(None);
                }
                self.force_render = true;
            }
            AuthorizationState::Connecting => {
                if self.authorization_message.is_some() {
                    self.renderer.set_status_message::<String>(None);
                }
                self.force_render = true;
            }
        }
    }

    fn tick_authorization(&mut self) {
        if matches!(self.authorization_state, AuthorizationState::Connecting)
            && self.connect_started_at.elapsed() >= AUTH_FALLBACK_WAIT
        {
            let fallback_message = if self.authorization_pending_hint {
                AUTH_WAIT_MESSAGE_INIT.to_string()
            } else {
                AUTH_WAIT_MESSAGE_SYNCING.to_string()
            };
            debug!(
                pending_hint = self.authorization_pending_hint,
                elapsed = ?self.connect_started_at.elapsed(),
                "transitioning to Waiting state"
            );
            self.set_authorization_state(AuthorizationState::Waiting, Some(fallback_message));
        }

        if matches!(self.authorization_state, AuthorizationState::Waiting)
            && self.authorization_last_tick.elapsed() >= AUTH_SPINNER_INTERVAL
        {
            self.authorization_spinner_index =
                (self.authorization_spinner_index + 1) % AUTH_SPINNER_FRAMES.len();
            self.authorization_last_tick = Instant::now();
            self.refresh_authorization_status();
        }

        if let AuthorizationState::Waiting = self.authorization_state {
            if let Some(started) = self.authorization_wait_started {
                let elapsed = started.elapsed();
                let hint_level = if elapsed >= AUTH_HINT_STAGE_TWO {
                    2
                } else if elapsed >= AUTH_HINT_STAGE_ONE {
                    1
                } else {
                    0
                };
                if hint_level != self.authorization_hint_level {
                    self.authorization_hint_level = hint_level;
                    let current = self
                        .authorization_message
                        .as_deref()
                        .unwrap_or(AUTH_WAIT_MESSAGE);
                    if current == AUTH_WAIT_MESSAGE || current == AUTH_WAIT_MESSAGE_INIT {
                        let replacement = match hint_level {
                            2 => AUTH_WAIT_HINT_TWO,
                            1 => AUTH_WAIT_HINT_ONE,
                            _ => AUTH_WAIT_MESSAGE,
                        };
                        self.authorization_message = Some(replacement.to_string());
                        self.refresh_authorization_status();
                    }
                }
            }
        }

        if let AuthorizationState::Approved = self.authorization_state {
            if let Some(started) = self.authorization_approved_since {
                if started.elapsed() >= AUTH_APPROVED_MESSAGE_DURATION
                    && self.authorization_message.is_some()
                {
                    self.authorization_message = None;
                    self.authorization_approved_since = None;
                    self.refresh_authorization_status();
                }
            }
        }
    }

    fn handle_authorization_signal(&mut self, signal: &str) -> bool {
        let trimmed = signal.trim();
        let Some(rest) = trimmed.strip_prefix("beach:status:") else {
            return false;
        };
        let (kind, detail) = match rest.split_once(' ') {
            Some((k, d)) => (k, d.trim()),
            None => (rest, ""),
        };
        match kind {
            "approval_pending" => {
                if self.subscription_id.is_some() {
                    return true;
                }
                let message = if detail.is_empty() {
                    AUTH_WAIT_MESSAGE.to_string()
                } else {
                    detail.to_string()
                };
                self.authorization_pending_hint = true;
                self.set_authorization_state(AuthorizationState::Waiting, Some(message));
                true
            }
            "approval_granted" => {
                let message = if detail.is_empty() {
                    AUTH_APPROVED_MESSAGE.to_string()
                } else {
                    detail.to_string()
                };
                self.authorization_pending_hint = false;
                self.set_authorization_state(AuthorizationState::Approved, Some(message));
                true
            }
            "approval_denied" => {
                let message = if detail.is_empty() {
                    AUTH_DENIED_MESSAGE.to_string()
                } else {
                    detail.to_string()
                };
                self.authorization_pending_hint = false;
                self.set_authorization_state(AuthorizationState::Denied, Some(message));
                true
            }
            _ => false,
        }
    }

    fn send_resize(&mut self, cols: u16, rows: u16) -> Result<(), ClientError> {
        let frame = WireClientFrame::Resize { cols, rows };
        let encoded = protocol::encode_client_frame_binary(&frame);
        telemetry::record_bytes("client_input_frames", encoded.len());
        self.transport
            .send_bytes(&encoded)
            .map_err(ClientError::Transport)?;
        debug!(target = "client::outgoing", cols, rows, "resize sent");
        Ok(())
    }

    fn handle_input_ack(&mut self, seq: Seq) {
        let now = Instant::now();
        let mut sent_at: Option<Instant> = None;
        if let Some(prediction) = self.pending_predictions.get_mut(&seq) {
            prediction.acked_at = Some(now);
            sent_at = Some(prediction.sent_at);
        }
        if self.pending_predictions.contains_key(&seq) {
            self.try_clear_prediction(seq, now);
        } else {
            self.renderer.clear_prediction_seq(seq);
        }
        if let Some(sent) = sent_at {
            self.record_prediction_ack(sent);
        }
        self.force_render = true;
        self.update_prediction_overlay();
    }

    fn register_prediction(&mut self, seq: Seq, bytes: &[u8]) {
        if !self.render_enabled || !self.predictive_input {
            return;
        }
        if bytes.len() > 32 {
            return;
        }
        if self.pending_predictions.len() > 256 {
            self.pending_predictions.clear();
            self.renderer.clear_all_predictions();
            self.reset_prediction_state();
        }
        let mut cursor_row = self.cursor_row;
        let mut cursor_col = self.cursor_col;
        let mut cursor_changed = false;
        let mut positions: Vec<PredictedPosition> = Vec::new();

        let mut push_position = |row: usize, col: usize, ch: char| {
            if let Some(existing) = positions
                .iter_mut()
                .find(|pos| pos.row == row && pos.col == col)
            {
                existing.ch = ch;
            } else {
                positions.push(PredictedPosition { row, col, ch });
            }
        };

        for &byte in bytes {
            match byte {
                b'\r' => {
                    if cursor_col != 0 {
                        cursor_col = 0;
                        cursor_changed = true;
                    }
                }
                b'\n' => {
                    cursor_row = cursor_row.saturating_add(1);
                    cursor_col = 0;
                    cursor_changed = true;
                }
                0x08 | 0x7f => {
                    let mut moved = false;
                    if cursor_col > 0 {
                        cursor_col = cursor_col.saturating_sub(1);
                        moved = true;
                    } else if cursor_row > 0 {
                        cursor_row = cursor_row.saturating_sub(1);
                        cursor_col = self
                            .renderer
                            .committed_row_width(cursor_row as u64)
                            .saturating_sub(1);
                        moved = true;
                    }
                    if moved {
                        let row = cursor_row;
                        let col = cursor_col;
                        self.renderer.add_prediction(row, col, seq, ' ');
                        push_position(row, col, ' ');
                        cursor_changed = true;
                    }
                }
                0x00..=0x1f => {}
                value => {
                    let ch = value as char;
                    let row = cursor_row;
                    let col = cursor_col;
                    self.renderer.add_prediction(row, col, seq, ch);
                    push_position(row, col, ch);
                    cursor_col = cursor_col.saturating_add(1);
                    cursor_changed = true;
                }
            }
        }

        if cursor_changed {
            self.cursor_row = cursor_row;
            self.cursor_col = cursor_col;
            self.sync_renderer_cursor();
        }

        if !positions.is_empty() {
            self.pending_predictions.insert(
                seq,
                PendingPrediction {
                    positions,
                    sent_at: Instant::now(),
                    acked_at: None,
                },
            );
            self.force_render = true;
        } else if cursor_changed {
            self.force_render = true;
        }

        self.update_prediction_overlay();
    }

    fn prune_acked_predictions(&mut self, now: Instant) {
        let mut expired: Vec<Seq> = Vec::new();
        for (&seq, prediction) in self.pending_predictions.iter_mut() {
            prediction
                .positions
                .retain(|pos| self.renderer.prediction_exists(pos.row, pos.col, seq));

            if let Some(acked_at) = prediction.acked_at {
                if now.saturating_duration_since(acked_at) >= PREDICTION_ACK_GRACE {
                    expired.push(seq);
                }
            }
        }

        if expired.is_empty() {
            return;
        }

        for seq in expired {
            self.try_clear_prediction(seq, now);
        }
    }

    fn try_clear_prediction(&mut self, seq: Seq, now: Instant) -> bool {
        if let Some(prediction) = self.pending_predictions.remove(&seq) {
            let overlay_gone = prediction
                .positions
                .iter()
                .all(|pos| !self.renderer.prediction_exists(pos.row, pos.col, seq));
            let committed = prediction
                .positions
                .iter()
                .all(|pos| self.renderer.cell_matches(pos.row, pos.col, pos.ch));
            if overlay_gone || committed {
                self.renderer.clear_prediction_seq(seq);
                self.force_render = true;
                self.prediction_last_quick_confirmation = Some(now);
                true
            } else {
                self.pending_predictions.insert(seq, prediction);
                false
            }
        } else {
            false
        }
    }

    fn row_has_predictions(&self, row: usize) -> bool {
        if self.renderer.predicted_row_width(row as u64) > 0 {
            return true;
        }
        self.pending_predictions
            .values()
            .any(|pending| pending.positions.iter().any(|pos| pos.row == row))
    }

    fn discard_predictions_from_column(&mut self, row: usize, col: usize) {
        let trimmed_renderer = self.renderer.shrink_row_to_column(row as u64, col);
        let mut trimmed_pending = false;
        for prediction in self.pending_predictions.values_mut() {
            let before = prediction.positions.len();
            prediction
                .positions
                .retain(|pos| !(pos.row == row && pos.col >= col));
            if prediction.positions.len() != before {
                trimmed_pending = true;
            }
        }
        if trimmed_pending {
            self.pending_predictions
                .retain(|_, pending| !pending.positions.is_empty());
        }
        if trimmed_renderer || trimmed_pending {
            self.force_render = true;
            self.update_prediction_overlay();
        }
    }

    fn update_prediction_overlay(&mut self) {
        let now = Instant::now();
        self.prune_acked_predictions(now);

        if !self.render_enabled || !self.predictive_input {
            self.renderer.set_predictions_visible(false);
            self.renderer.set_prediction_flagging(false);
            return;
        }
        let mut glitch_trigger = self.prediction_glitch_trigger;
        for pending in self.pending_predictions.values() {
            let age = now.saturating_duration_since(pending.sent_at);
            if age >= PREDICTION_GLITCH_FLAG_THRESHOLD {
                glitch_trigger = PREDICTION_GLITCH_REPAIR_COUNT * 2;
                break;
            } else if age >= PREDICTION_GLITCH_THRESHOLD
                && glitch_trigger < PREDICTION_GLITCH_REPAIR_COUNT
            {
                glitch_trigger = PREDICTION_GLITCH_REPAIR_COUNT;
            }
        }
        self.prediction_glitch_trigger = glitch_trigger;

        let srtt = self.prediction_srtt_ms.unwrap_or(0.0);

        if srtt > PREDICTION_FLAG_TRIGGER_HIGH_MS
            || self.prediction_glitch_trigger > PREDICTION_GLITCH_REPAIR_COUNT
        {
            self.prediction_flagging = true;
        } else if self.prediction_flagging
            && srtt <= PREDICTION_FLAG_TRIGGER_LOW_MS
            && self.prediction_glitch_trigger <= PREDICTION_GLITCH_REPAIR_COUNT
        {
            self.prediction_flagging = false;
        }

        if srtt > PREDICTION_SRTT_TRIGGER_HIGH_MS || self.prediction_glitch_trigger > 0 {
            self.prediction_srtt_trigger = true;
        } else if self.prediction_srtt_trigger
            && srtt <= PREDICTION_SRTT_TRIGGER_LOW_MS
            && self.pending_predictions.is_empty()
            && !self.renderer.has_active_predictions()
        {
            self.prediction_srtt_trigger = false;
        }

        let has_predictions =
            !self.pending_predictions.is_empty() || self.renderer.has_active_predictions();

        let overlay_visible = self.predictive_input
            && (has_predictions
                || self.prediction_srtt_trigger
                || self.prediction_glitch_trigger > 0);
        let underline = overlay_visible
            && (self.prediction_flagging
                || self.prediction_glitch_trigger > PREDICTION_GLITCH_REPAIR_COUNT);

        self.renderer.set_predictions_visible(overlay_visible);
        self.renderer.set_prediction_flagging(underline);
    }

    fn record_prediction_ack(&mut self, sent_at: Instant) {
        let now = Instant::now();
        let rtt = now.saturating_duration_since(sent_at);
        let sample_ms = rtt.as_secs_f64() * 1000.0;
        self.prediction_srtt_ms = Some(match self.prediction_srtt_ms {
            Some(previous) => previous + (sample_ms - previous) * PREDICTION_SRTT_ALPHA,
            None => sample_ms,
        });

        if self.prediction_glitch_trigger > 0 && rtt < PREDICTION_GLITCH_THRESHOLD {
            let allow_decay = match self.prediction_last_quick_confirmation {
                Some(last) => {
                    now.saturating_duration_since(last) >= PREDICTION_GLITCH_REPAIR_MIN_INTERVAL
                }
                None => true,
            };
            if allow_decay {
                self.prediction_glitch_trigger = self.prediction_glitch_trigger.saturating_sub(1);
                self.prediction_last_quick_confirmation = Some(now);
            }
        }

        self.update_prediction_overlay();
    }

    fn reset_prediction_state(&mut self) {
        self.prediction_srtt_ms = None;
        self.prediction_srtt_trigger = false;
        self.prediction_flagging = false;
        self.prediction_glitch_trigger = 0;
        self.prediction_last_quick_confirmation = None;
        self.renderer.set_predictions_visible(false);
        self.renderer.set_prediction_flagging(false);
    }

    fn setup_tui(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            self.force_render = true;
            self.renderer.mark_dirty();
            return Ok(());
        }
        enable_raw_mode()
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        terminal.hide_cursor().ok();
        if let Ok(area) = terminal.size() {
            self.renderer.on_resize(area.width, area.height);
        }
        self.renderer.mark_dirty();
        self.force_render = true;
        self.tui = Some(terminal);
        Ok(())
    }

    fn teardown_tui(&mut self) -> Result<(), ClientError> {
        if !self.render_enabled {
            return Ok(());
        }
        if let Some(mut terminal) = self.tui.take() {
            terminal.show_cursor().ok();
            terminal
                .clear()
                .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        }
        disable_raw_mode()
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        let mut stdout = io::stdout();
        execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)
            .map_err(|err| ClientError::Transport(TransportError::Setup(err.to_string())))?;
        Ok(())
    }

    #[cfg(test)]
    pub fn test_row_text(&self, row: u64) -> Option<String> {
        self.renderer.row_text_for_test(row)
    }

    #[cfg(test)]
    pub fn test_rows_text(&self, start: u64, end: u64) -> Vec<Option<String>> {
        (start..end)
            .map(|row| self.renderer.row_text_for_test(row))
            .collect()
    }
}

fn decode_wire_cell(cell: u64) -> (char, Option<u32>) {
    let packed = PackedCell::from(cell);
    let (ch, style_id) = unpack_cell(packed);
    if style_id == StyleId::DEFAULT {
        (ch, None)
    } else {
        (ch, Some(style_id.0))
    }
}

#[derive(Clone, Debug)]
struct CopyModeState {
    anchor: SelectionPosition,
    cursor: SelectionPosition,
    selection_active: bool,
    selection_mode: SelectionMode,
    mode: CopyModeKeySet,
    pending_input: Option<CopyModePendingInput>,
    last_search: Option<CopyModeSearch>,
}

impl CopyModeState {
    fn new(anchor: SelectionPosition, mode: CopyModeKeySet) -> Self {
        Self {
            anchor,
            cursor: anchor,
            selection_active: false,
            selection_mode: SelectionMode::Character,
            mode,
            pending_input: None,
            last_search: None,
        }
    }

    fn begin_selection(&mut self, mode: SelectionMode) {
        self.selection_active = true;
        self.selection_mode = mode;
        self.anchor = self.cursor;
    }

    fn clear_selection(&mut self) {
        self.selection_active = false;
    }

    fn toggle_selection(&mut self) {
        if self.selection_active {
            self.selection_active = false;
        } else {
            self.begin_selection(SelectionMode::Character);
        }
    }
}

#[derive(Clone, Debug)]
struct PredictedPosition {
    row: usize,
    col: usize,
    ch: char,
}

struct PendingPrediction {
    positions: Vec<PredictedPosition>,
    sent_at: Instant,
    acked_at: Option<Instant>,
}

enum CursorHint {
    Exact(usize, usize),
}

fn encode_key_event(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            let mut bytes = Vec::new();
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    bytes.push((lower as u8 - b'a') + 1);
                } else {
                    return None;
                }
            } else {
                bytes.push(c as u8);
            }
            Some(bytes)
        }
        KeyCode::Enter => Some(vec![b'\n']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}

fn is_copy_shortcut(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(c) => {
            (key.modifiers.contains(KeyModifiers::SUPER) && c.eq_ignore_ascii_case(&'c'))
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                    && c.eq_ignore_ascii_case(&'c'))
        }
        KeyCode::Insert => key.modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}

fn copy_mode_command_for_key(
    mode: CopyModeKeySet,
    selection_active: bool,
    key: &KeyEvent,
) -> Option<CopyModeCommand> {
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            match c.to_ascii_lowercase() {
                'y' => return Some(CopyModeCommand::CopySelectionAndExit),
                'c' => return Some(CopyModeCommand::ClearSelection),
                'v' => return Some(CopyModeCommand::SetMode(CopyModeKeySet::Vi)),
                'e' => return Some(CopyModeCommand::SetMode(CopyModeKeySet::Emacs)),
                ']' | '}' => return Some(CopyModeCommand::Cancel),
                _ => {}
            }
        }
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            if c.eq_ignore_ascii_case(&'g') {
                return Some(CopyModeCommand::Cancel);
            }
        }
    }

    match key.code {
        KeyCode::Esc => Some(CopyModeCommand::Cancel),
        KeyCode::Enter => Some(CopyModeCommand::CopySelectionAndExit),
        KeyCode::Up => Some(CopyModeCommand::Move { rows: -1, cols: 0 }),
        KeyCode::Down => Some(CopyModeCommand::Move { rows: 1, cols: 0 }),
        KeyCode::Left => Some(CopyModeCommand::Move { rows: 0, cols: -1 }),
        KeyCode::Right => Some(CopyModeCommand::Move { rows: 0, cols: 1 }),
        KeyCode::PageUp => Some(CopyModeCommand::Page { delta: -1 }),
        KeyCode::PageDown => Some(CopyModeCommand::Page { delta: 1 }),
        KeyCode::Home => Some(CopyModeCommand::MoveToLineStart),
        KeyCode::End => Some(CopyModeCommand::MoveToLineEnd),
        _ => match mode {
            CopyModeKeySet::Vi => copy_mode_command_vi(selection_active, key),
            CopyModeKeySet::Emacs => copy_mode_command_emacs(selection_active, key),
        },
    }
}

fn copy_mode_command_vi(selection_active: bool, key: &KeyEvent) -> Option<CopyModeCommand> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return match c.to_ascii_lowercase() {
                'b' => Some(CopyModeCommand::Page { delta: -1 }),
                'f' => Some(CopyModeCommand::Page { delta: 1 }),
                'u' => Some(CopyModeCommand::HalfPage { delta: -1 }),
                'd' => Some(CopyModeCommand::HalfPage { delta: 1 }),
                'c' => Some(CopyModeCommand::Cancel),
                'v' => Some(CopyModeCommand::SetSelectionMode(SelectionMode::Block)),
                _ => None,
            };
        }
    }

    match key.code {
        KeyCode::Char('G') => Some(CopyModeCommand::JumpBottom),
        KeyCode::Char('N') => Some(CopyModeCommand::RepeatLastSearch(
            CopyModeSearchDirection::Backward,
        )),
        KeyCode::Char('0') => Some(CopyModeCommand::MoveToLineStart),
        KeyCode::Char('$') => Some(CopyModeCommand::MoveToLineEnd),
        KeyCode::Char('^') => Some(CopyModeCommand::MoveToLineStart),
        KeyCode::Char('V') => Some(CopyModeCommand::SetSelectionMode(SelectionMode::Line)),
        KeyCode::Char(' ') => {
            if selection_active {
                Some(CopyModeCommand::ToggleSelection)
            } else {
                Some(CopyModeCommand::BeginSelection)
            }
        }
        KeyCode::Char('/') => Some(CopyModeCommand::Search(CopyModeSearchDirection::Forward)),
        KeyCode::Char('?') => Some(CopyModeCommand::Search(CopyModeSearchDirection::Backward)),
        KeyCode::Char(c) => {
            let lower = c.to_ascii_lowercase();
            match lower {
                'h' => Some(CopyModeCommand::Move { rows: 0, cols: -1 }),
                'j' => Some(CopyModeCommand::Move { rows: 1, cols: 0 }),
                'k' => Some(CopyModeCommand::Move { rows: -1, cols: 0 }),
                'l' => Some(CopyModeCommand::Move { rows: 0, cols: 1 }),
                'g' => Some(CopyModeCommand::JumpTop),
                'w' => Some(CopyModeCommand::MoveWord(WordMotion::NextStart)),
                'e' => Some(CopyModeCommand::MoveWord(WordMotion::NextEnd)),
                'b' => Some(CopyModeCommand::MoveWord(WordMotion::PrevStart)),
                'y' => Some(CopyModeCommand::CopySelectionAndExit),
                'v' => Some(CopyModeCommand::ToggleSelection),
                'q' => Some(CopyModeCommand::Cancel),
                'n' => Some(CopyModeCommand::RepeatLastSearch(
                    CopyModeSearchDirection::Forward,
                )),
                _ => None,
            }
        }
        _ => None,
    }
}

fn copy_mode_command_emacs(selection_active: bool, key: &KeyEvent) -> Option<CopyModeCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        if let KeyCode::Char(c) = key.code {
            return match c.to_ascii_lowercase() {
                'f' => Some(CopyModeCommand::Move { rows: 0, cols: 1 }),
                'b' => Some(CopyModeCommand::Move { rows: 0, cols: -1 }),
                'n' => Some(CopyModeCommand::Move { rows: 1, cols: 0 }),
                'p' => Some(CopyModeCommand::Move { rows: -1, cols: 0 }),
                'a' => Some(CopyModeCommand::MoveToLineStart),
                'e' => Some(CopyModeCommand::MoveToLineEnd),
                'v' => Some(CopyModeCommand::Page { delta: 1 }),
                'w' => Some(CopyModeCommand::CopySelectionAndExit),
                'y' => Some(CopyModeCommand::CopySelection),
                's' => Some(CopyModeCommand::Search(CopyModeSearchDirection::Forward)),
                'r' => Some(CopyModeCommand::Search(CopyModeSearchDirection::Backward)),
                'g' => Some(CopyModeCommand::Cancel),
                ' ' => {
                    if selection_active {
                        Some(CopyModeCommand::ToggleSelection)
                    } else {
                        Some(CopyModeCommand::BeginSelection)
                    }
                }
                _ => None,
            };
        }
    }

    if alt {
        if let KeyCode::Char(c) = key.code {
            return match c.to_ascii_lowercase() {
                'f' => Some(CopyModeCommand::MoveWord(WordMotion::NextStart)),
                'b' => Some(CopyModeCommand::MoveWord(WordMotion::PrevStart)),
                'd' => Some(CopyModeCommand::MoveWord(WordMotion::NextEnd)),
                'v' => Some(CopyModeCommand::Page { delta: -1 }),
                'w' => Some(CopyModeCommand::CopySelection),
                'y' => Some(CopyModeCommand::CopySelectionAndExit),
                _ => None,
            };
        }
    }

    match key.code {
        KeyCode::Char(' ') => {
            if selection_active {
                Some(CopyModeCommand::ToggleSelection)
            } else {
                Some(CopyModeCommand::BeginSelection)
            }
        }
        KeyCode::Char('y') => Some(CopyModeCommand::CopySelection),
        _ => None,
    }
}

fn default_copy_mode_keyset() -> CopyModeKeySet {
    match env::var(COPY_MODE_KEYSET_ENV) {
        Ok(value) if value.eq_ignore_ascii_case("emacs") => CopyModeKeySet::Emacs,
        _ => CopyModeKeySet::Vi,
    }
}

fn reverse_search_direction(direction: CopyModeSearchDirection) -> CopyModeSearchDirection {
    match direction {
        CopyModeSearchDirection::Forward => CopyModeSearchDirection::Backward,
        CopyModeSearchDirection::Backward => CopyModeSearchDirection::Forward,
    }
}

fn find_next_word_start_in_line(chars: &[char], start_col: Option<usize>) -> Option<usize> {
    if chars.is_empty() {
        return None;
    }
    let mut idx = start_col.and_then(|col| col.checked_add(1)).unwrap_or(0);
    if let Some(col) = start_col {
        if col < chars.len() && is_word_char(chars[col]) {
            idx = col.saturating_add(1);
            while idx < chars.len() && is_word_char(chars[idx]) {
                idx += 1;
            }
        }
    }
    if idx > chars.len() {
        idx = chars.len();
    }
    while idx < chars.len() && !is_word_char(chars[idx]) {
        idx += 1;
    }
    if idx < chars.len() { Some(idx) } else { None }
}

fn find_next_word_end_in_line(chars: &[char], start_col: Option<usize>) -> Option<usize> {
    if chars.is_empty() {
        return None;
    }
    let mut idx = match start_col {
        Some(col) if col < chars.len() => col,
        Some(_) => return None,
        None => 0,
    };
    if !is_word_char(chars[idx]) {
        while idx < chars.len() && !is_word_char(chars[idx]) {
            idx += 1;
        }
        if idx >= chars.len() {
            return None;
        }
    }
    let mut end = idx;
    while end + 1 < chars.len() && is_word_char(chars[end + 1]) {
        end += 1;
    }
    Some(end)
}

fn find_prev_word_start_in_line(chars: &[char], start_col: Option<usize>) -> Option<usize> {
    if chars.is_empty() {
        return None;
    }
    let mut idx = match start_col {
        Some(0) => return None,
        Some(col) if col > chars.len() => chars.len().saturating_sub(1),
        Some(col) => col.saturating_sub(1),
        None => chars.len().saturating_sub(1),
    };
    while idx > 0 && !is_word_char(chars[idx]) {
        idx -= 1;
    }
    if !is_word_char(chars[idx]) {
        return None;
    }
    while idx > 0 && is_word_char(chars[idx - 1]) {
        idx -= 1;
    }
    Some(idx)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn encode_mouse_event(mouse: &MouseEvent) -> Option<Vec<u8>> {
    let (mut code, suffix) = match mouse.kind {
        MouseEventKind::Down(button) => (mouse_button_code(button)?, 'M'),
        MouseEventKind::Up(_) => (3, 'm'),
        MouseEventKind::Drag(button) => (mouse_button_code(button)? + 32, 'M'),
        MouseEventKind::ScrollUp => (64, 'M'),
        MouseEventKind::ScrollDown => (65, 'M'),
        _ => return None,
    };

    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if mouse.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }

    let column = mouse.column.saturating_add(1);
    let row = mouse.row.saturating_add(1);
    let sequence = format!("\u{1b}[<{code};{column};{row}{suffix}");
    Some(sequence.into_bytes())
}

fn mouse_button_code(button: MouseButton) -> Option<u16> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use crate::cache::terminal::PackedCell;
    use crate::protocol::{Lane, LaneBudgetFrame, SyncConfigFrame};
    use crate::transport::{
        Transport, TransportError, TransportId, TransportKind, TransportMessage, TransportPair,
    };
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct NullTransport;

    impl Transport for NullTransport {
        fn kind(&self) -> TransportKind {
            TransportKind::Ipc
        }

        fn id(&self) -> TransportId {
            TransportId(0)
        }

        fn peer(&self) -> TransportId {
            TransportId(0)
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

        fn recv(&self, _timeout: Duration) -> Result<TransportMessage, TransportError> {
            Err(TransportError::Timeout)
        }

        fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct RecordingTransport {
        sent: Mutex<Vec<Vec<u8>>>,
    }

    impl RecordingTransport {
        fn take(&self) -> Vec<Vec<u8>> {
            self.sent.lock().expect("sent mutex").drain(..).collect()
        }
    }

    impl Transport for RecordingTransport {
        fn kind(&self) -> TransportKind {
            TransportKind::Ipc
        }

        fn id(&self) -> TransportId {
            TransportId(0)
        }

        fn peer(&self) -> TransportId {
            TransportId(0)
        }

        fn send(&self, _message: TransportMessage) -> Result<(), TransportError> {
            Ok(())
        }

        fn send_text(&self, text: &str) -> Result<u64, TransportError> {
            self.send_bytes(text.as_bytes())
        }

        fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
            self.sent.lock().expect("sent mutex").push(bytes.to_vec());
            Ok(bytes.len() as u64)
        }

        fn recv(&self, _timeout: Duration) -> Result<TransportMessage, TransportError> {
            Err(TransportError::Timeout)
        }

        fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
            Ok(None)
        }
    }

    fn pack_char(ch: char) -> u64 {
        let packed = PackedCell::from_raw((ch as u32 as u64) << 32);
        packed.into()
    }

    fn pack_text_row(absolute_row: u32, label: u32) -> WireUpdate {
        let text = format!("Line {label}: Test");
        let cells = text.chars().map(pack_char).collect();
        WireUpdate::Row {
            row: absolute_row,
            seq: (absolute_row as u64).saturating_add(1),
            cells,
        }
    }

    fn pack_row(absolute_row: u32, seq: u64, text: &str) -> WireUpdate {
        WireUpdate::Row {
            row: absolute_row,
            seq,
            cells: text.chars().map(pack_char).collect(),
        }
    }

    fn pack_row_segment(absolute_row: u32, start_col: u32, seq: u64, text: &str) -> WireUpdate {
        WireUpdate::RowSegment {
            row: absolute_row,
            start_col,
            seq,
            cells: text.chars().map(pack_char).collect(),
        }
    }

    fn pack_rect(
        row_start: u32,
        row_end: u32,
        col_start: u32,
        col_end: u32,
        seq: u64,
        ch: char,
    ) -> WireUpdate {
        WireUpdate::Rect {
            rows: [row_start, row_end],
            cols: [col_start, col_end],
            seq,
            cell: pack_char(ch),
        }
    }

    fn new_client() -> TerminalClient {
        TerminalClient::new(Arc::new(NullTransport)).with_render(false)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn rect_of_spaces_does_not_push_cursor() {
        let mut client = new_client();
        client.renderer.ensure_size(1, 80);

        client.apply_wire_update(&pack_rect(0, 1, 0, 80, 1, ' '));
        assert_eq!(client.cursor_row, 0);
        assert_eq!(client.cursor_col, 0);

        let prompt = "[user@host ~]$ ";
        client.apply_wire_update(&pack_row_segment(0, 0, 2, prompt));
        assert_eq!(client.cursor_row, 0);
        assert_eq!(client.cursor_col, prompt.chars().count());
    }

    #[test]
    fn handshake_snapshot_rect_preserves_initial_cursor_column() {
        use crate::protocol::{
            CursorFrame, FEATURE_CURSOR_SYNC, Lane, LaneBudgetFrame, SyncConfigFrame,
        };

        let mut client = new_client();
        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![LaneBudgetFrame {
                lane: Lane::Foreground,
                max_updates: 128,
            }],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 128,
        };

        client
            .handle_host_frame(WireHostFrame::Hello {
                subscription: 1,
                max_seq: 0,
                config: sync_config,
                features: FEATURE_CURSOR_SYNC,
            })
            .expect("hello");

        client
            .handle_host_frame(WireHostFrame::Grid {
                cols: 80,
                history_rows: 0,
                base_row: 0,
                viewport_rows: Some(24),
            })
            .expect("grid");

        let bottom_row = client.cursor_row as u32;
        assert_eq!(client.cursor_col, 0);

        client
            .handle_host_frame(WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 1,
                has_more: false,
                updates: vec![pack_rect(0, bottom_row + 1, 0, 80, 1, ' ')],
                cursor: None,
            })
            .expect("snapshot");

        assert_eq!(client.cursor_col, 0);

        client
            .handle_host_frame(WireHostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            })
            .expect("snapshot complete");

        let prompt = "$ ";
        let prompt_len = prompt.chars().count();
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 2,
                has_more: false,
                updates: vec![pack_row_segment(bottom_row, 0, 2, prompt)],
                cursor: Some(CursorFrame {
                    row: bottom_row,
                    col: prompt_len as u32,
                    seq: 2,
                    visible: true,
                    blink: true,
                }),
            })
            .expect("delta prompt");

        assert_eq!(client.cursor_col, prompt_len);
    }

    #[test]
    fn predictive_space_ack_clears_overlay() {
        let mut client = new_client();
        client.render_enabled = true;
        client.predictive_input = true;
        client.renderer.ensure_size(1, 4);

        client.register_prediction(1, b" ");
        assert!(client.renderer.has_active_predictions());
        assert!(!client.pending_predictions.is_empty());

        client.handle_input_ack(1);
        assert!(!client.renderer.has_active_predictions());
        assert!(client.pending_predictions.is_empty());
    }

    #[test]
    fn ctrl_u_clears_line_and_resets_cursor() {
        let mut client = new_client();
        let prompt = "$ ";
        client.apply_wire_update(&pack_row_segment(0, 0, 1, prompt));
        let typed = "$ hello";
        client.apply_wire_update(&pack_row_segment(0, 0, 2, typed));
        assert_eq!(client.cursor_col, typed.chars().count());

        let blanks = " ".repeat(typed.chars().count());
        client.apply_wire_update(&pack_row_segment(0, 0, 3, &blanks));
        assert_eq!(client.cursor_col, 0);
    }

    #[test]
    fn vi_word_motions_navigate_between_words() {
        let mut client = new_client();
        client.renderer.ensure_size(3, 32);
        client
            .renderer
            .apply_row_from_text(0, 1, "alpha beta  gamma");
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 0, col: 0 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        client.process_copy_mode_key(&key(KeyCode::Char('w'), KeyModifiers::NONE));
        let cursor = client.copy_mode.as_ref().unwrap().cursor;
        assert_eq!(cursor.row, 0);
        assert_eq!(cursor.col, 6);

        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::NONE));
        let cursor = client.copy_mode.as_ref().unwrap().cursor;
        assert_eq!(cursor.row, 0);
        assert_eq!(cursor.col, 0);

        client.process_copy_mode_key(&key(KeyCode::Char('e'), KeyModifiers::NONE));
        let cursor = client.copy_mode.as_ref().unwrap().cursor;
        assert_eq!(cursor.row, 0);
        assert_eq!(cursor.col, 4);
    }

    #[test]
    fn vi_half_page_motions_respect_viewport() {
        let mut client = new_client();
        client.renderer.on_resize(80, 8);
        client.renderer.ensure_size(12, 32);
        for row in 0..12 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 6, col: 0 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        client.process_copy_mode_key(&key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        let cursor = client.copy_mode.as_ref().unwrap().cursor;
        assert_eq!(cursor.row, 3);

        client.process_copy_mode_key(&key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        let cursor = client.copy_mode.as_ref().unwrap().cursor;
        assert_eq!(cursor.row, 6);
    }

    #[test]
    fn mouse_scroll_selection_tracks_actual_viewport_delta() {
        let mut client = new_client();
        client.renderer.on_resize(80, 4);
        client.renderer.ensure_size(6, 32);
        for row in 0..6 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.renderer.scroll_to_tail();

        client.enter_copy_mode();
        {
            let state = client.copy_mode.as_mut().unwrap();
            state.begin_selection(SelectionMode::Character);
        }

        client.move_copy_cursor(-1, 0);
        let initial_row = client.copy_mode.as_ref().unwrap().cursor.row;
        assert_eq!(initial_row, 4);

        let before_top = client.renderer.viewport_top();
        client.handle_mouse_scroll(-20);
        let after_up = client.copy_mode.as_ref().unwrap().cursor.row;
        assert_eq!(before_top, 4);
        assert_eq!(after_up, 0, "scroll up should move cursor by actual delta");

        client.handle_mouse_scroll(20);
        if let Some(state) = client.copy_mode.as_ref() {
            assert_eq!(
                state.cursor.row, initial_row,
                "reverse scroll should restore cursor row"
            );
        } else {
            assert!(client.renderer.is_following_tail());
        }
    }

    #[test]
    fn mouse_drag_edges_autoscroll_copy_mode() {
        let mut client = new_client();
        client.renderer.on_resize(80, 4);
        client.renderer.ensure_size(12, 32);
        for row in 0..12 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.renderer.scroll_to_tail();

        client.enter_copy_mode();
        {
            let state = client.copy_mode.as_mut().unwrap();
            state.begin_selection(SelectionMode::Character);
        }

        let initial_cursor = client.copy_mode.as_ref().unwrap().cursor.row;
        assert_eq!(
            initial_cursor,
            client.renderer.viewport_top() + client.renderer.viewport_height() as u64 - 1,
        );

        let drag_top = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        client.handle_mouse_primary_drag(&drag_top);
        let after_up = client.copy_mode.as_ref().unwrap().cursor.row;
        assert!(
            after_up < initial_cursor,
            "cursor should move upward when dragging at top edge",
        );

        let bottom_row = client.renderer.viewport_height().saturating_sub(1) as u16;
        let drag_bottom = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 0,
            row: bottom_row,
            modifiers: KeyModifiers::NONE,
        };
        client.handle_mouse_primary_drag(&drag_bottom);
        if let Some(state) = client.copy_mode.as_ref() {
            assert!(
                state.cursor.row >= after_up,
                "cursor should move downward when dragging at bottom edge",
            );
        } else {
            assert!(client.renderer.is_following_tail());
        }
    }

    #[test]
    fn page_up_enters_copy_mode_and_scrolls() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(40, 32);
        for row in 0..40 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.renderer.scroll_to_tail();

        let before_top = client.renderer.viewport_top();
        assert!(client.copy_mode.is_none());

        client.process_copy_mode_key(&key(KeyCode::PageUp, KeyModifiers::NONE));

        assert!(client.copy_mode.is_some(), "PageUp should enter copy mode");
        let viewport = client.renderer.viewport_height() as u64;
        let after_top = client.renderer.viewport_top();
        assert_eq!(
            after_top,
            before_top.saturating_sub(viewport),
            "viewport should move up by one page"
        );
        let cursor_row = client.copy_mode.as_ref().unwrap().cursor.row;
        assert_eq!(
            cursor_row,
            after_top + viewport.saturating_sub(1),
            "cursor should land on bottom row of new viewport"
        );
    }

    #[test]
    fn page_up_clamps_to_history_base() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(40, 32);
        for row in 0..40 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.renderer.set_base_row(10);
        client.renderer.scroll_to_tail();

        let before_top = client.renderer.viewport_top();
        client.process_copy_mode_key(&key(KeyCode::PageUp, KeyModifiers::NONE));

        let viewport = client.renderer.viewport_height() as u64;
        let expected_top = before_top
            .saturating_sub(viewport)
            .max(client.renderer.base_row());
        assert_eq!(client.renderer.viewport_top(), expected_top);
        assert!(client.copy_mode.is_some());
    }

    #[test]
    fn page_down_exits_copy_mode_at_tail() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(40, 32);
        for row in 0..40 {
            let text = format!("line {row:02}");
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &text);
        }
        client.renderer.scroll_to_tail();

        client.process_copy_mode_key(&key(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(client.copy_mode.is_some());

        client.process_copy_mode_key(&key(KeyCode::PageDown, KeyModifiers::NONE));

        assert!(
            client.copy_mode.is_none(),
            "PageDown at tail should exit copy mode"
        );
        assert!(client.renderer.is_following_tail());
    }

    #[test]
    fn ctrl_b_bracket_enters_copy_mode() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(10, 32);
        for row in 0..10 {
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &format!("line {row:02}"));
        }
        client.renderer.scroll_to_tail();

        assert!(client.copy_mode.is_none());
        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(
            client.copy_mode.is_none(),
            "prefix should not enter copy mode yet"
        );

        client.process_copy_mode_key(&key(KeyCode::Char('['), KeyModifiers::NONE));
        assert!(
            client.copy_mode.is_some(),
            "Ctrl-B [ should enter copy mode"
        );
    }

    #[test]
    fn ctrl_b_page_up_pages_copy_mode() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(30, 32);
        for row in 0..30 {
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &format!("line {row:02}"));
        }
        client.renderer.scroll_to_tail();

        let before_top = client.renderer.viewport_top();

        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::CONTROL));
        client.process_copy_mode_key(&key(KeyCode::PageUp, KeyModifiers::NONE));

        assert!(client.copy_mode.is_some());
        let viewport = client.renderer.viewport_height() as u64;
        let after_top = client.renderer.viewport_top();
        assert_eq!(after_top, before_top.saturating_sub(viewport));
    }

    #[test]
    fn ctrl_b_timeout_expires_prefix() {
        let mut client = new_client();
        client.renderer.on_resize(80, 6);
        client.renderer.ensure_size(10, 32);
        for row in 0..10 {
            client
                .renderer
                .apply_row_from_text(row, (row + 1) as u64, &format!("line {row:02}"));
        }
        client.renderer.scroll_to_tail();

        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::CONTROL));
        client.tmux_prefix_started_at =
            Some(Instant::now() - TMUX_PREFIX_TIMEOUT - Duration::from_millis(1));

        client.process_copy_mode_key(&key(KeyCode::Char('['), KeyModifiers::NONE));
        assert!(
            client.copy_mode.is_none(),
            "expired prefix should not enter copy mode"
        );
    }

    #[test]
    fn ctrl_b_right_bracket_pastes_clipboard() {
        clipboard::set("line1\nline2").unwrap();
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        client.subscription_id = Some(1);
        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::CONTROL));
        client.process_copy_mode_key(&key(KeyCode::Char(']'), KeyModifiers::NONE));

        let frames = transport.take();
        assert_eq!(frames.len(), 1, "expected single paste frame");
        let frame = protocol::decode_client_frame_binary(&frames[0]).expect("decode paste frame");
        match frame {
            WireClientFrame::Input { data, .. } => {
                assert_eq!(data, b"line1\nline2".to_vec());
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    #[test]
    fn ctrl_b_right_bracket_handles_empty_clipboard() {
        clipboard::clear();
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        client.subscription_id = Some(1);
        client.process_copy_mode_key(&key(KeyCode::Char('b'), KeyModifiers::CONTROL));
        client.process_copy_mode_key(&key(KeyCode::Char(']'), KeyModifiers::NONE));

        assert!(
            transport.take().is_empty(),
            "no frames expected when clipboard empty"
        );
    }

    #[test]
    fn vi_ctrl_v_switches_to_block_selection() {
        let mut client = new_client();
        client.renderer.ensure_size(2, 8);
        client.renderer.apply_row_from_text(0, 1, "abcd");
        client.renderer.apply_row_from_text(1, 2, "efgh");
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 1, col: 3 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        // Begin selection, move to the opposite corner, then toggle block mode.
        client.process_copy_mode_key(&key(KeyCode::Char('v'), KeyModifiers::NONE));
        client.process_copy_mode_key(&key(KeyCode::Char('k'), KeyModifiers::NONE));
        for _ in 0..3 {
            client.process_copy_mode_key(&key(KeyCode::Char('h'), KeyModifiers::NONE));
        }
        client.process_copy_mode_key(&key(KeyCode::Char('v'), KeyModifiers::CONTROL));

        let selected = client.renderer.selection_text().unwrap();
        assert_eq!(selected, "abcd\nefgh");
    }

    #[test]
    fn vi_shift_v_enters_line_selection() {
        let mut client = new_client();
        client.renderer.ensure_size(2, 16);
        client.renderer.apply_row_from_text(0, 1, "alpha");
        client.renderer.apply_row_from_text(1, 2, "beta");
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 1, col: 2 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        client.process_copy_mode_key(&key(KeyCode::Char('V'), KeyModifiers::SHIFT));
        client.process_copy_mode_key(&key(KeyCode::Char('k'), KeyModifiers::NONE));

        let selected = client.renderer.selection_text().unwrap();
        assert_eq!(selected, "alpha\nbeta");
    }

    #[test]
    fn copy_mode_cmd_shortcut_exits_when_no_selection() {
        let mut client = new_client();
        client.renderer.ensure_size(1, 32);
        client.renderer.apply_row_from_text(0, 1, "sample text");
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 0, col: 0 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        assert!(client.copy_mode.is_some());
        client.process_copy_mode_key(&key(KeyCode::Char('c'), KeyModifiers::SUPER));
        assert!(client.copy_mode.is_none());
    }

    #[test]
    fn vi_q_exits_copy_mode() {
        let mut client = new_client();
        client.renderer.ensure_size(1, 16);
        client.renderer.apply_row_from_text(0, 1, "hello world");
        client.copy_mode = Some(CopyModeState::new(
            SelectionPosition { row: 0, col: 5 },
            CopyModeKeySet::Vi,
        ));
        client.renderer.set_follow_tail(false);
        client.update_copy_mode_status();

        client.process_copy_mode_key(&key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(client.copy_mode.is_none());
    }

    fn seed_request(
        client: &mut TerminalClient,
        id: u64,
        start: u64,
        count: u32,
        more_expected: bool,
    ) {
        client.pending_backfills.push(BackfillRequestState {
            id,
            start,
            end: start.saturating_add(count as u64),
            issued_at: Instant::now(),
            more_expected,
        });
    }

    #[test_timeout::timeout]
    fn history_backfill_trim_regression_repro() {
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![LaneBudgetFrame {
                lane: Lane::Foreground,
                max_updates: 128,
            }],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 128,
        };
        client
            .handle_host_frame(WireHostFrame::Hello {
                subscription: 1,
                max_seq: 0,
                config: sync_config.clone(),
                features: 0,
            })
            .expect("hello");
        client
            .handle_host_frame(WireHostFrame::Grid {
                viewport_rows: Some(24),
                cols: 80,
                history_rows: 600,
                base_row: 0,
            })
            .expect("grid");

        let seeds: Vec<WireUpdate> = (0..24).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 24,
                has_more: false,
                updates: seeds,
                cursor: None,
            })
            .expect("snapshot");
        client
            .handle_host_frame(WireHostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            })
            .expect("snapshot complete");

        client
            .maybe_request_backfill()
            .expect("request backfill after snapshot");

        let initial_requests = transport.take();
        assert_eq!(
            initial_requests.len(),
            1,
            "expected initial backfill request"
        );

        let first_state = client
            .pending_backfills
            .last()
            .expect("pending backfill missing");
        let request_id = first_state.id;
        let start_row = first_state.start;
        let count = (first_state.end - first_state.start) as u32;

        let mut updates = Vec::new();
        updates.push(WireUpdate::Trim {
            start: 89,
            count: 333,
            seq: 426,
        });
        for offset in 0..22u32 {
            let absolute = 426u32 + offset;
            updates.push(pack_row(
                absolute,
                5000 + u64::from(offset),
                &format!("Line {absolute}: Test"),
            ));
        }

        client
            .handle_host_frame(WireHostFrame::HistoryBackfill {
                subscription: 1,
                request_id,
                start_row,
                count,
                updates,
                more: false,
                cursor: None,
            })
            .expect("history backfill");

        assert!(
            client.renderer_base_row() >= 400,
            "expected initial backfill to advance base"
        );

        client
            .handle_host_frame(WireHostFrame::HistoryBackfill {
                subscription: 1,
                request_id: request_id + 1,
                start_row,
                count,
                updates: Vec::new(),
                more: false,
                cursor: None,
            })
            .expect("empty history backfill");

        client
            .maybe_request_backfill()
            .expect("no further backfill after empty response");
        assert!(
            client.pending_backfills.is_empty(),
            "client should not re-request trimmed tail; pending={:?}",
            client.pending_backfills
        );

        assert!(
            client
                .empty_tail_ranges
                .iter()
                .any(|range| range.start == start_row),
            "empty tail range should be tracked"
        );

        // Force retry window to elapse and simulate tail growth to trigger re-request.
        if let Some(range) = client.empty_tail_ranges.first_mut() {
            range.recorded_at = range
                .recorded_at
                .checked_sub(Duration::from_millis(200))
                .unwrap_or(range.recorded_at);
            range.highest_at = Some(start_row);
        }
        let gap = client
            .renderer
            .first_unloaded_range(BACKFILL_LOOKAHEAD_ROWS);
        assert!(gap.is_some(), "expected gap for retry");
        let (gap_start, gap_span) = gap.expect("gap start/len");
        client.highest_loaded_row = Some(gap_start.saturating_add(gap_span as u64));

        client
            .maybe_request_backfill()
            .expect("retry suppression after tail advances");

        let retried = transport.take();
        for frame in retried {
            let decoded =
                crate::protocol::decode_client_frame_binary(&frame).expect("decode client frame");
            if let WireClientFrame::RequestBackfill {
                start_row: retried_start,
                ..
            } = decoded
            {
                assert_ne!(
                    retried_start, start_row,
                    "unexpected repeat backfill for trimmed range"
                );
            }
        }

        assert!(
            client.renderer_base_row() >= 400,
            "base row should stay at or above trimmed origin"
        );
    }

    #[test_timeout::timeout]
    fn tail_alignment_end_to_end_regression() {
        use crate::protocol::{self, HostFrame};

        let pair = TransportPair::new(TransportKind::Ipc);
        let server: Arc<dyn Transport> = Arc::from(pair.server);
        let client_transport: Arc<dyn Transport> = Arc::from(pair.client);
        let mut client = TerminalClient::new(client_transport.clone()).with_render(false);

        fn send_frame(server: &Arc<dyn Transport>, frame: HostFrame) {
            let bytes = protocol::encode_host_frame_binary(&frame);
            server.send_bytes(&bytes).expect("send host frame");
        }

        fn deliver_next(transport: &Arc<dyn Transport>, client: &mut TerminalClient) {
            let message = transport
                .recv(Duration::from_millis(200))
                .expect("client recv");
            let bytes = match message.payload {
                Payload::Binary(bytes) => bytes,
                Payload::Text(text) => text.into_bytes(),
            };
            let frame = protocol::decode_host_frame_binary(&bytes).expect("decode host frame");
            client.handle_host_frame(frame).expect("handle host frame");
        }

        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![LaneBudgetFrame {
                lane: Lane::Foreground,
                max_updates: 128,
            }],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 128,
        };

        send_frame(
            &server,
            HostFrame::Hello {
                subscription: 1,
                max_seq: 0,
                config: sync_config.clone(),
                features: 0,
            },
        );
        deliver_next(&client_transport, &mut client);

        send_frame(
            &server,
            HostFrame::Grid {
                viewport_rows: Some(24),
                cols: 80,
                history_rows: 600,
                base_row: 0,
            },
        );
        deliver_next(&client_transport, &mut client);

        let seeds: Vec<WireUpdate> = (0..24).map(|row| pack_text_row(row, row + 1)).collect();
        send_frame(
            &server,
            HostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 24,
                has_more: false,
                updates: seeds,
                cursor: None,
            },
        );
        deliver_next(&client_transport, &mut client);

        send_frame(
            &server,
            HostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            },
        );
        deliver_next(&client_transport, &mut client);

        client
            .maybe_request_backfill()
            .expect("request backfill after snapshot");

        let first_state = client
            .pending_backfills
            .last()
            .expect("pending backfill missing");
        let request_id = first_state.id;
        let start_row = first_state.start;
        let count = (first_state.end - first_state.start) as u32;

        let mut updates = Vec::new();
        updates.push(WireUpdate::Trim {
            start: 89,
            count: 333,
            seq: 426,
        });
        for offset in 0..22u32 {
            let absolute = 426 + offset;
            updates.push(pack_row(
                absolute,
                5000 + offset as u64,
                &format!("Line {absolute}: Test"),
            ));
        }

        send_frame(
            &server,
            HostFrame::HistoryBackfill {
                subscription: 1,
                request_id,
                start_row,
                count,
                updates,
                more: true,
                cursor: None,
            },
        );
        deliver_next(&client_transport, &mut client);
        assert!(client.renderer_base_row() >= 400, "base row should advance");

        let pending = client
            .pending_backfills
            .iter()
            .find(|req| req.id == request_id)
            .expect("backfill should remain pending while more=true");
        assert!(
            pending.more_expected,
            "expected more chunks after partial backfill"
        );

        send_frame(
            &server,
            HostFrame::HistoryBackfill {
                subscription: 1,
                request_id,
                start_row,
                count,
                updates: Vec::new(),
                more: false,
                cursor: None,
            },
        );
        deliver_next(&client_transport, &mut client);

        client
            .maybe_request_backfill()
            .expect("no further backfill after empty response");
        assert!(
            client.pending_backfills.is_empty(),
            "client should not re-request trimmed tail; pending={:?}",
            client.pending_backfills
        );

        assert!(
            client.renderer_base_row() >= 400,
            "base row should stay at or above trimmed origin"
        );
    }

    #[test_timeout::timeout]
    fn restored_session_tail_renders_after_empty_backfill() {
        use crate::protocol::{self, ClientFrame as WireClientFrame, HostFrame as WireHostFrame};

        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![LaneBudgetFrame {
                lane: Lane::Foreground,
                max_updates: 128,
            }],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 128,
        };

        client
            .handle_host_frame(WireHostFrame::Hello {
                subscription: 1,
                max_seq: 0,
                config: sync_config.clone(),
                features: 0,
            })
            .expect("hello");
        client
            .handle_host_frame(WireHostFrame::Grid {
                viewport_rows: Some(58),
                cols: 80,
                history_rows: 600,
                base_row: 0,
            })
            .expect("grid");

        let seeds: Vec<WireUpdate> = (0..58).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 58,
                has_more: false,
                updates: seeds,
                cursor: None,
            })
            .expect("snapshot");
        client
            .handle_host_frame(WireHostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            })
            .expect("snapshot complete");

        client
            .maybe_request_backfill()
            .expect("initial history backfill request");

        let mut frames = transport.take();
        assert_eq!(frames.len(), 1, "expected initial backfill request frame");
        let WireClientFrame::RequestBackfill {
            request_id,
            start_row,
            count,
            ..
        } = protocol::decode_client_frame_binary(&frames[0]).expect("decode initial request")
        else {
            panic!("expected RequestBackfill for initial history");
        };

        let mut updates = Vec::new();
        updates.push(WireUpdate::Trim {
            start: 89,
            count: 333,
            seq: 426,
        });
        for offset in 0..22u32 {
            let absolute = 426 + offset;
            updates.push(pack_row(
                absolute,
                5000 + offset as u64,
                &format!("Line {absolute}: Test"),
            ));
        }
        client
            .handle_host_frame(WireHostFrame::HistoryBackfill {
                subscription: 1,
                request_id,
                start_row,
                count,
                updates,
                more: false,
                cursor: None,
            })
            .expect("apply trimmed history");
        assert!(
            client.renderer_base_row() >= 400,
            "trimmed history should advance base row"
        );

        client.highest_loaded_row = Some(576);
        client.has_loaded_rows = true;

        client
            .maybe_request_backfill()
            .expect("tail follow-up request");
        frames = transport.take();

        let (tail_request_id, tail_start, tail_count) = if let Some(frame_bytes) = frames.first() {
            assert_eq!(
                frames.len(),
                1,
                "expected at most one tail backfill request"
            );
            let WireClientFrame::RequestBackfill {
                request_id: id,
                start_row,
                count,
                ..
            } = protocol::decode_client_frame_binary(frame_bytes).expect("decode tail request")
            else {
                panic!("expected RequestBackfill for tail");
            };
            (id, start_row, count)
        } else {
            let fallback_id = request_id + 1;
            let fallback_start = 447u64;
            let fallback_count = 16;
            seed_request(
                &mut client,
                fallback_id,
                fallback_start,
                fallback_count,
                false,
            );
            (fallback_id, fallback_start, fallback_count)
        };
        assert!(
            tail_start >= 400,
            "tail request should target trimmed region"
        );
        let trimmed_end = tail_start + u64::from(tail_count);

        client
            .handle_host_frame(WireHostFrame::HistoryBackfill {
                subscription: 1,
                request_id: tail_request_id,
                start_row: tail_start,
                count: tail_count,
                updates: Vec::new(),
                more: false,
                cursor: None,
            })
            .expect("apply empty tail backfill");

        client
            .maybe_request_backfill()
            .expect("suppress repeat tail request after empty reply");
        frames = transport.take();
        if let Some(frame_bytes) = frames.first() {
            assert_eq!(frames.len(), 1, "expected at most one post-empty request");
            let WireClientFrame::RequestBackfill { start_row, .. } =
                protocol::decode_client_frame_binary(frame_bytes)
                    .expect("decode post-empty request")
            else {
                panic!("expected RequestBackfill frame");
            };
            assert!(
                start_row >= trimmed_end,
                "unexpected backfill retry after empty reply"
            );
        }

        let mut delta_updates = Vec::new();
        for idx in 0..150u32 {
            let absolute = 422 + idx;
            let label = idx + 1;
            delta_updates.push(pack_row(
                absolute,
                8000 + idx as u64,
                &format!("Line {label}: Test"),
            ));
        }
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 9500,
                has_more: false,
                updates: delta_updates,
                cursor: None,
            })
            .expect("apply tail delta");

        client.highest_loaded_row = Some((422 + 149) as u64);

        client
            .maybe_request_backfill()
            .expect("no tail backfill after delta");
        assert!(
            transport.take().is_empty(),
            "unexpected request after delta application"
        );

        client.renderer.scroll_to_tail();
        let lines = client.renderer.visible_lines();
        let non_blank = lines.iter().filter(|line| !line.trim().is_empty()).count();
        assert!(non_blank > 0, "tail viewport should contain rendered rows");
        assert!(
            lines.iter().any(|line| line.contains("Line 150: Test")),
            "tail should render latest line"
        );
        if let Some(highest) = client.highest_loaded_row {
            assert!(
                client
                    .renderer
                    .first_gap_between(client.renderer.base_row(), highest.saturating_add(1))
                    .is_none(),
                "no pending gaps expected after delta"
            );
        }
        assert!(
            client.empty_tail_ranges.is_empty(),
            "empty tail ranges should clear"
        );
        assert!(
            client
                .pending_backfills
                .iter()
                .all(|req| req.start >= trimmed_end),
            "no pending backfill should target trimmed tail"
        );
    }

    #[test_timeout::timeout]
    fn history_backfill_loads_rows_across_sparse_chunks() {
        let transport: Arc<dyn Transport> = Arc::new(NullTransport);
        let mut client = TerminalClient::new(transport).with_render(false);

        let base: u32 = 12000;

        client.subscription_id = Some(1);
        client.known_base_row = Some(base as u64);
        client.renderer.ensure_size(400, 80);

        seed_request(&mut client, 1, 0, 64, true);
        let updates: Vec<WireUpdate> = (0..24)
            .map(|idx| pack_text_row(base + idx, idx + 1))
            .collect();
        client
            .handle_history_backfill(1, 1, 0, 64, updates, true)
            .expect("first chunk");

        client
            .handle_history_backfill(1, 1, 64, 64, Vec::new(), true)
            .expect("second chunk empty");
        client
            .handle_history_backfill(1, 1, 128, 64, Vec::new(), true)
            .expect("third chunk empty");
        client
            .handle_history_backfill(1, 1, 192, 64, Vec::new(), false)
            .expect("final chunk empty");

        let delta_updates: Vec<WireUpdate> = (0..150)
            .map(|idx| pack_text_row(base + idx, idx + 1))
            .collect();
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 150,
                has_more: false,
                updates: delta_updates,
                cursor: None,
            })
            .expect("apply tail delta");

        seed_request(&mut client, 2, 24, 28, false);
        let updates: Vec<WireUpdate> = (24..52)
            .map(|idx| pack_text_row(base + idx, idx + 1))
            .collect();
        client
            .handle_history_backfill(1, 2, 24, 28, updates, false)
            .expect("backfill range 24-51");

        seed_request(&mut client, 3, 37, 256, true);
        let updates: Vec<WireUpdate> = (37..101)
            .map(|idx| pack_text_row(base + idx, idx + 1))
            .collect();
        client
            .handle_history_backfill(1, 3, 37, 64, updates, true)
            .expect("range 37-100");

        let updates: Vec<WireUpdate> = (101..158)
            .map(|idx| pack_text_row(base + idx, idx + 1))
            .collect();
        client
            .handle_history_backfill(1, 3, 101, 64, updates, true)
            .expect("range 101-157");

        client
            .handle_history_backfill(1, 3, 165, 64, Vec::new(), true)
            .expect("empty tail chunk");
        client
            .handle_history_backfill(1, 3, 229, 64, Vec::new(), false)
            .expect("final tail chunk");

        for row in 0..150u64 {
            let text = client
                .test_row_text(base as u64 + row)
                .unwrap_or_default()
                .trim_end()
                .to_string();
            let row_label = row + 1;
            assert!(
                text.contains(&format!("Line {row_label}")),
                "row {row} missing expected text, got '{text}'"
            );
        }
    }

    #[test_timeout::timeout]
    fn row_segment_overwrites_shrinks_row() {
        let transport: Arc<dyn Transport> = Arc::new(NullTransport);
        let mut client = TerminalClient::new(transport).with_render(false);

        client.subscription_id = Some(1);
        client.known_base_row = Some(5000);
        client.renderer.ensure_size(20, 80);

        // Seed row with a long command line.
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 1,
                has_more: false,
                updates: vec![pack_row(
                    5000,
                    1,
                    "world                                  for",
                )],
                cursor: None,
            })
            .expect("seed row");

        // Apply a shorter update that should replace the trailing text.
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 2,
                has_more: false,
                updates: vec![pack_row_segment(5000, 0, 2, "world")],
                cursor: None,
            })
            .expect("apply shorter row");

        let text = client
            .test_row_text(5000)
            .unwrap_or_default()
            .trim_end()
            .to_string();

        assert_eq!(text, "world", "row retained stale suffix: '{text}'");
    }

    #[test_timeout::timeout]
    fn follow_tail_prefers_loaded_rows_after_empty_tail_backfill() {
        let transport: Arc<dyn Transport> = Arc::new(NullTransport);
        let mut client = TerminalClient::new(transport).with_render(false);

        client.subscription_id = Some(1);
        client.renderer.ensure_size(320, 80);
        client.renderer.on_resize(80, 12);
        client.renderer.set_follow_tail(true);

        client.finalize_backfill_range(250, 280, &[]);
        client.renderer.scroll_to_tail();

        let updates: Vec<WireUpdate> = (140..145)
            .map(|row| {
                let text = format!("Tail row {row:03}");
                pack_row(row as u32, row as u64 + 10_000, &text)
            })
            .collect();

        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 20_000,
                has_more: false,
                updates,
                cursor: None,
            })
            .expect("apply tail delta");

        client.renderer.scroll_to_tail();
        let lines = client.renderer.visible_lines();
        let all_pending = lines.iter().all(|line| line.chars().all(|ch| ch == '·'));
        assert!(!all_pending, "tail view stuck on pending rows: {lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("Tail row")),
            "tail view missing loaded content: {lines:?}"
        );
    }

    #[test_timeout::timeout]
    fn streaming_deltas_do_not_trigger_tail_backfill_requests() {
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![
                LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 500,
                },
                LaneBudgetFrame {
                    lane: Lane::Recent,
                    max_updates: 500,
                },
                LaneBudgetFrame {
                    lane: Lane::History,
                    max_updates: 500,
                },
            ],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 500,
        };

        client
            .handle_host_frame(WireHostFrame::Hello {
                subscription: 1,
                max_seq: 4,
                config: sync_config.clone(),
                features: 0,
            })
            .expect("hello");
        client
            .handle_host_frame(WireHostFrame::Grid {
                viewport_rows: Some(24),
                cols: 80,
                history_rows: 24,
                base_row: 0,
            })
            .expect("grid");
        let snapshot_updates: Vec<WireUpdate> =
            (0..4).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 4,
                has_more: false,
                updates: snapshot_updates,
                cursor: None,
            })
            .expect("snapshot");
        client
            .handle_host_frame(WireHostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            })
            .expect("snapshot complete");

        client
            .maybe_request_backfill()
            .expect("maybe request after snapshot");
        assert_no_backfill_requests(&transport.take());

        let tail_updates: Vec<WireUpdate> =
            (4..150).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 300,
                has_more: false,
                updates: tail_updates,
                cursor: None,
            })
            .expect("delta burst");

        client
            .maybe_request_backfill()
            .expect("maybe request after burst");
        assert_no_backfill_requests(&transport.take());

        for offset in 0..150u64 {
            let expected_label = offset + 1;
            let expected = format!("Line {expected_label}");
            let row_text = client
                .test_row_text(offset)
                .unwrap_or_default()
                .trim_end()
                .to_string();
            assert!(
                row_text.contains(&expected),
                "row {offset} missing expected text, got '{row_text}'"
            );
        }
    }

    #[test_timeout::timeout]
    fn follow_tail_does_not_request_history_after_handshake() {
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        let sync_config = SyncConfigFrame {
            snapshot_budgets: vec![
                LaneBudgetFrame {
                    lane: Lane::Foreground,
                    max_updates: 500,
                },
                LaneBudgetFrame {
                    lane: Lane::Recent,
                    max_updates: 500,
                },
                LaneBudgetFrame {
                    lane: Lane::History,
                    max_updates: 500,
                },
            ],
            delta_budget: 512,
            heartbeat_ms: 250,
            initial_snapshot_lines: 500,
        };

        client
            .handle_host_frame(WireHostFrame::Hello {
                subscription: 1,
                max_seq: 4,
                config: sync_config.clone(),
                features: 0,
            })
            .expect("hello");
        client
            .handle_host_frame(WireHostFrame::Grid {
                viewport_rows: Some(24),
                cols: 80,
                history_rows: 154,
                base_row: 0,
            })
            .expect("grid");

        let snapshot_updates: Vec<WireUpdate> =
            (0..24).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Snapshot {
                subscription: 1,
                lane: Lane::Foreground,
                watermark: 24,
                has_more: false,
                updates: snapshot_updates,
                cursor: None,
            })
            .expect("snapshot");
        client
            .handle_host_frame(WireHostFrame::SnapshotComplete {
                subscription: 1,
                lane: Lane::Foreground,
            })
            .expect("snapshot complete");

        transport.take();

        let tail_updates: Vec<WireUpdate> =
            (24..150).map(|row| pack_text_row(row, row + 1)).collect();
        client
            .handle_host_frame(WireHostFrame::Delta {
                subscription: 1,
                watermark: 300,
                has_more: false,
                updates: tail_updates,
                cursor: None,
            })
            .expect("delta burst");

        client
            .maybe_request_backfill()
            .expect("no backfill while following tail");

        let frames = transport.take();
        if !frames.is_empty() {
            assert_eq!(frames.len(), 1, "expected at most one backfill request");
            let payload = &frames[0];
            if let Ok(WireClientFrame::RequestBackfill { start_row, .. }) =
                protocol::decode_client_frame_binary(payload)
            {
                assert!(
                    start_row >= 150,
                    "unexpected backfill start {start_row}; expected tail range"
                );
            } else {
                panic!("unexpected client frame while following tail");
            }
        }
    }

    #[test_timeout::timeout]
    fn gap_detection_prefers_lower_history_after_tail_burst() {
        let transport: Arc<RecordingTransport> = Arc::new(RecordingTransport::default());
        let mut client = TerminalClient::new(transport.clone()).with_render(false);

        client.subscription_id = Some(1);
        client.known_base_row = Some(0);
        client.renderer.ensure_size(400, 80);
        client.renderer.set_follow_tail(false);

        for row in 0..24u64 {
            let label = row as usize + 1;
            client
                .renderer
                .apply_row_from_text(row as usize, row, &format!("Line {label}: Test"));
        }
        for row in 24..112u64 {
            client.renderer.mark_row_missing(row);
        }
        for row in 112..150u64 {
            let label = row as usize + 1;
            client.renderer.apply_row_from_text(
                row as usize,
                1000 + row,
                &format!("Line {label}: Test"),
            );
        }
        client.highest_loaded_row = Some(149);
        client.has_loaded_rows = true;
        client.pending_backfills.clear();
        client.last_tail_backfill_start = None;
        client.last_gap_backfill_start = None;
        client.last_backfill_request_at = None;
        transport.take();

        client
            .maybe_request_backfill()
            .expect("trigger gap backfill");

        let frames = transport.take();
        for frame in frames {
            if let WireClientFrame::RequestBackfill { start_row, .. } =
                protocol::decode_client_frame_binary(&frame).expect("decode backfill frame")
            {
                assert!(
                    start_row >= 24,
                    "unexpected backfill request for trimmed gap at row {start_row}"
                );
            }
        }
    }

    fn assert_no_backfill_requests(frames: &[Vec<u8>]) {
        for bytes in frames {
            if let Ok(frame) = protocol::decode_client_frame_binary(bytes) {
                if matches!(frame, WireClientFrame::RequestBackfill { .. }) {
                    panic!("unexpected backfill request frame: {frame:?}");
                }
            }
        }
    }
}

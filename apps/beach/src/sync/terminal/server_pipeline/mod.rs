// Extracted sync pipeline utilities

use crate::cache::Seq;
use crate::cache::terminal::{PackedCell, StyleId, TerminalGrid, unpack_cell};
use crate::model::terminal::diff::{CacheUpdate, HistoryTrim, RowSnapshot, StyleDefinition};
use crate::protocol::{
    self, ClientFrame as WireClientFrame, CursorFrame, FEATURE_CURSOR_SYNC, HostFrame,
    Lane as WireLane, LaneBudgetFrame as WireLaneBudget, SyncConfigFrame as WireSyncConfig,
    Update as WireUpdate,
};
use crate::sync::terminal::{TerminalDeltaStream, TerminalSync};
use crate::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use crate::telemetry;
use crate::telemetry::PerfGuard;
use crate::transport::terminal::negotiation::{SharedTransport, TransportSupervisor};
use crate::transport::{Transport, TransportError, TransportId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{Level, debug, info, trace, warn};

pub(crate) const MAX_TRANSPORT_FRAME_BYTES: usize = 48 * 1024;
const MAX_UPDATES_PER_FRAME: usize = 64;
pub(crate) const MAX_BACKFILL_ROWS_PER_REQUEST: u32 = 256;
pub(crate) const SERVER_BACKFILL_CHUNK_ROWS: u32 = 64;
pub(crate) const SERVER_BACKFILL_THROTTLE: Duration = Duration::from_millis(50);

pub(crate) type ForwardTransport = (Arc<dyn Transport>, Option<Arc<TransportSupervisor>>);

fn map_lane(lane: PriorityLane) -> WireLane {
    match lane {
        PriorityLane::Foreground => WireLane::Foreground,
        PriorityLane::Recent => WireLane::Recent,
        PriorityLane::History => WireLane::History,
    }
}

pub(crate) struct BackfillChunk {
    pub updates: Vec<CacheUpdate>,
    pub attempted: u32,
    pub delivered: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct BackfillCommand {
    pub transport_id: TransportId,
    pub subscription: u64,
    pub request_id: u64,
    pub start_row: u64,
    pub count: u32,
}

#[derive(Debug)]
struct BackfillJob {
    subscription: u64,
    request_id: u64,
    next_row: u64,
    end_row: u64,
}

pub(crate) fn collect_backfill_chunk(
    grid: &TerminalGrid,
    start_row: u64,
    max_rows: u32,
) -> BackfillChunk {
    if max_rows == 0 {
        return BackfillChunk {
            updates: Vec::new(),
            attempted: 0,
            delivered: 0,
        };
    }

    let cols = grid.cols();
    if cols == 0 {
        return BackfillChunk {
            updates: Vec::new(),
            attempted: max_rows,
            delivered: 0,
        };
    }

    let mut updates = Vec::new();
    let mut buffer: Vec<u64> = vec![0; cols];
    let mut style_ids: HashSet<StyleId> = HashSet::new();
    let mut delivered = 0u32;

    let base_offset = grid.row_offset();
    let mut effective_start = start_row;
    if start_row < base_offset {
        let diff = base_offset - start_row;
        if let (Ok(start), Ok(count)) = (usize::try_from(start_row), usize::try_from(diff)) {
            updates.push(CacheUpdate::Trim(HistoryTrim::new(start, count)));
            trace!(
                target = "sync::backfill",
                start_row, base_offset, count, "emitting trim for backfill"
            );
        } else {
            trace!(
                target = "sync::backfill",
                start_row, base_offset, diff, "trim conversion overflow"
            );
        }
        effective_start = base_offset;
    }

    trace!(
        target = "sync::backfill",
        start_row, effective_start, max_rows, base_offset, cols, "collecting backfill chunk"
    );

    let default_cell = TerminalGrid::pack_char_with_style(' ', StyleId::DEFAULT);
    let first_id = grid.first_row_id();
    let last_id = grid.last_row_id();
    trace!(
        target = "sync::backfill",
        start_row,
        effective_start,
        max_rows,
        base_offset,
        cols,
        first_id,
        last_id,
        total_rows = grid.rows(),
        "collecting backfill chunk"
    );

    for offset in 0..max_rows as u64 {
        let absolute = effective_start.saturating_add(offset);
        let Some(index) = grid.index_of_row(absolute) else {
            trace!(target = "sync::backfill", absolute, "row missing from grid");
            continue;
        };

        if grid.snapshot_row_into(index, &mut buffer[..cols]).is_err() {
            continue;
        }

        if tracing::enabled!(Level::TRACE) && offset < 4 {
            let preview: String = buffer
                .iter()
                .map(|cell| unpack_cell(PackedCell::from_raw(*cell)).0)
                .collect();
            trace!(
                target = "sync::backfill",
                row = absolute,
                text = %preview.trim_end_matches(' ')
            );
        }

        let mut max_seq = 0;
        let mut packed_cells: Vec<PackedCell> = Vec::with_capacity(cols);
        for (col, raw_cell) in buffer.iter().enumerate().take(cols) {
            if let Some(snapshot) = grid.get_cell_relaxed(index, col) {
                max_seq = max_seq.max(snapshot.seq);
            }
            let packed = PackedCell::from_raw(*raw_cell);
            let (_, style_id) = unpack_cell(packed);
            style_ids.insert(style_id);
            packed_cells.push(packed);
        }

        if max_seq == 0
            && packed_cells
                .iter()
                .all(|cell| u64::from(*cell) == u64::from(default_cell))
        {
            trace!(
                target = "sync::backfill",
                row = absolute,
                "skipping default row with no seq"
            );
            continue;
        }

        updates.push(CacheUpdate::Row(RowSnapshot::new(
            absolute as usize,
            max_seq,
            packed_cells,
        )));
        delivered = delivered.saturating_add(1);
    }

    if delivered > 0 {
        let style_table = grid.style_table.clone();
        for style_id in style_ids {
            if let Some(style) = style_table.get(style_id) {
                updates.push(CacheUpdate::Style(StyleDefinition::new(
                    style_id,
                    effective_start,
                    style,
                )));
            }
        }
    }

    BackfillChunk {
        updates,
        attempted: max_rows,
        delivered,
    }
}

pub(crate) fn host_frame_label(frame: &HostFrame) -> &'static str {
    match frame {
        HostFrame::Heartbeat { .. } => "heartbeat",
        HostFrame::Hello { .. } => "hello",
        HostFrame::Grid { .. } => "grid",
        HostFrame::Snapshot { .. } => "snapshot",
        HostFrame::SnapshotComplete { .. } => "snapshot_complete",
        HostFrame::Delta { .. } => "delta",
        HostFrame::HistoryBackfill { .. } => "history_backfill",
        HostFrame::Cursor { .. } => "cursor",
        HostFrame::InputAck { .. } => "input_ack",
        HostFrame::Shutdown => "shutdown",
    }
}

pub(crate) fn client_frame_label(frame: &WireClientFrame) -> &'static str {
    match frame {
        WireClientFrame::Input { .. } => "input",
        WireClientFrame::Resize { .. } => "resize",
        WireClientFrame::RequestBackfill { .. } => "request_backfill",
        WireClientFrame::ViewportCommand { .. } => "viewport_command",
        WireClientFrame::Unknown => "unknown",
    }
}

pub(crate) fn send_host_frame(
    transport: &Arc<dyn Transport>,
    frame: HostFrame,
) -> Result<(), TransportError> {
    let encode_start = Instant::now();
    let frame_label = host_frame_label(&frame);
    if tracing::enabled!(Level::TRACE) {
        match &frame {
            HostFrame::Delta {
                updates, watermark, ..
            } => {
                let trim_count = updates
                    .iter()
                    .filter(|update| matches!(update, crate::protocol::Update::Trim { .. }))
                    .count();
                if trim_count > 0 {
                    trace!(
                        target = "sync::transport",
                        frame = frame_label,
                        trims = trim_count,
                        watermark,
                        "sending delta with trims"
                    );
                }
            }
            HostFrame::HistoryBackfill {
                updates,
                request_id,
                start_row,
                count,
                more,
                ..
            } => {
                let trim_count = updates
                    .iter()
                    .filter(|update| matches!(update, crate::protocol::Update::Trim { .. }))
                    .count();
                if trim_count > 0 {
                    trace!(
                        target = "sync::transport",
                        frame = frame_label,
                        trims = trim_count,
                        request_id,
                        start_row,
                        count,
                        more,
                        "sending history backfill with trims"
                    );
                }
            }
            _ => {}
        }
    }
    let bytes = protocol::encode_host_frame_binary(&frame);
    let elapsed = encode_start.elapsed();
    match &frame {
        HostFrame::Snapshot { .. } => telemetry::record_duration("sync_encode_snapshot", elapsed),
        HostFrame::Delta { .. } => telemetry::record_duration("sync_encode_delta", elapsed),
        _ => telemetry::record_duration("sync_encode_frame", elapsed),
    }
    match transport.send_bytes(&bytes) {
        Ok(sequence) => {
            if tracing::enabled!(Level::TRACE) {
                trace!(
                    target = "sync::transport",
                    transport_id = transport.id().0,
                    transport = ?transport.kind(),
                    frame = frame_label,
                    payload_len = bytes.len(),
                    sequence,
                    "host frame sent"
                );
            }
            Ok(())
        }
        Err(err) => {
            debug!(
                target = "sync::transport",
                transport_id = transport.id().0,
                transport = ?transport.kind(),
                frame = frame_label,
                error = %err,
                "failed to send host frame"
            );
            Err(err)
        }
    }
}

pub(crate) fn send_snapshot_frames_chunked(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    lane: PriorityLane,
    watermark: Seq,
    has_more: bool,
    batch: PreparedUpdateBatch,
) -> Result<(), TransportError> {
    let wire_lane = map_lane(lane);
    send_chunked_updates(
        transport,
        batch,
        has_more,
        |chunk_updates, chunk_has_more, cursor| HostFrame::Snapshot {
            subscription: subscription.0,
            lane: wire_lane,
            watermark,
            has_more: chunk_has_more,
            updates: chunk_updates,
            cursor,
        },
    )
}

pub(crate) fn send_delta_frames_chunked(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    watermark: Seq,
    has_more: bool,
    batch: PreparedUpdateBatch,
) -> Result<(), TransportError> {
    send_chunked_updates(
        transport,
        batch,
        has_more,
        |chunk_updates, chunk_has_more, cursor| HostFrame::Delta {
            subscription: subscription.0,
            watermark,
            has_more: chunk_has_more,
            updates: chunk_updates,
            cursor,
        },
    )
}

pub(crate) fn send_chunked_updates<F>(
    transport: &Arc<dyn Transport>,
    batch: PreparedUpdateBatch,
    final_has_more: bool,
    mut build_frame: F,
) -> Result<(), TransportError>
where
    F: FnMut(Vec<WireUpdate>, bool, Option<CursorFrame>) -> HostFrame,
{
    if batch.updates.is_empty() {
        let frame = build_frame(Vec::new(), final_has_more, batch.cursor);
        return send_host_frame(transport, frame);
    }

    let mut remaining: VecDeque<WireUpdate> = batch.updates.into();
    let mut chunk: Vec<WireUpdate> = Vec::new();
    let mut cursor_pending = batch.cursor;

    while let Some(update) = remaining.pop_front() {
        chunk.push(update);
        loop {
            let more_updates_pending = !remaining.is_empty();
            let chunk_has_more = more_updates_pending || final_has_more;
            let cursor_frame = cursor_pending.clone();
            let frame = build_frame(chunk.clone(), chunk_has_more, cursor_frame.clone());
            let encoded_len = protocol::encode_host_frame_binary(&frame).len();

            if encoded_len > MAX_TRANSPORT_FRAME_BYTES && chunk.len() > 1 {
                let overflow = chunk.pop().expect("chunk entry exists");
                let chunk_cursor = cursor_pending.clone();
                let chunk_frame = build_frame(chunk.clone(), true, chunk_cursor.clone());
                let chunk_len = protocol::encode_host_frame_binary(&chunk_frame).len();
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len = chunk_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending chunked host frame"
                );
                send_host_frame(transport, chunk_frame)?;
                if chunk_cursor.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                chunk.push(overflow);
                continue;
            }

            if encoded_len > MAX_TRANSPORT_FRAME_BYTES {
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending oversized single-update frame"
                );
                send_host_frame(transport, frame)?;
                if cursor_frame.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            if chunk.len() >= MAX_UPDATES_PER_FRAME {
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending chunked host frame"
                );
                send_host_frame(transport, frame)?;
                if cursor_frame.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            if !more_updates_pending {
                let final_cursor = cursor_pending.clone();
                let final_frame = build_frame(chunk.clone(), final_has_more, final_cursor.clone());
                let final_len = protocol::encode_host_frame_binary(&final_frame).len();
                trace!(
                    target = "sync::transport",
                    chunk_updates = chunk.len(),
                    encoded_len = final_len,
                    limit = MAX_TRANSPORT_FRAME_BYTES,
                    "sending final chunked host frame"
                );
                send_host_frame(transport, final_frame)?;
                if final_cursor.is_some() {
                    cursor_pending = None;
                }
                chunk.clear();
                break;
            }

            break;
        }

        if chunk.is_empty() {
            continue;
        }
    }

    if !chunk.is_empty() {
        let final_cursor = cursor_pending.clone();
        let final_frame = build_frame(chunk.clone(), final_has_more, final_cursor.clone());
        let encoded_len = protocol::encode_host_frame_binary(&final_frame).len();
        trace!(
            target = "sync::transport",
            chunk_updates = chunk.len(),
            encoded_len,
            limit = MAX_TRANSPORT_FRAME_BYTES,
            "sending trailing chunked host frame"
        );
        send_host_frame(transport, final_frame)?;
        if final_cursor.is_some() {}
    }

    Ok(())
}

pub(crate) struct TimelineDeltaStream {
    history: Mutex<VecDeque<CacheUpdate>>,
    latest: AtomicU64,
    capacity: usize,
}

impl TimelineDeltaStream {
    pub(crate) fn new() -> Self {
        Self {
            history: Mutex::new(VecDeque::with_capacity(1024)),
            latest: AtomicU64::new(0),
            capacity: 8192,
        }
    }

    pub(crate) fn record(&self, update: &CacheUpdate) {
        self.latest.store(update.seq(), Ordering::Relaxed);
        let mut history = self.history.lock().unwrap();
        history.push_back(update.clone());
        while history.len() > self.capacity {
            history.pop_front();
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TransmitterCache {
    cols: usize,
    rows: HashMap<usize, Vec<u64>>,
    styles: HashMap<u32, (u32, u32, u8)>,
    cursor: Option<CursorFrame>,
}

impl TransmitterCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reset(&mut self, cols: usize) {
        self.cols = cols;
        self.rows.clear();
        self.styles.clear();
        self.cursor = None;
    }

    pub(crate) fn apply_updates(
        &mut self,
        updates: &[CacheUpdate],
        dedupe: bool,
    ) -> PreparedUpdateBatch {
        let mut out = Vec::with_capacity(updates.len());
        let mut next_cursor: Option<CursorFrame> = None;
        for update in updates {
            match update {
                CacheUpdate::Row(row) => {
                    let cells: Vec<u64> = row.cells.iter().map(|c| (*c).into()).collect();
                    let changed = if dedupe {
                        self.rows
                            .get(&row.row)
                            .map(|existing| existing != &cells)
                            .unwrap_or(true)
                    } else {
                        true
                    };
                    self.cols = self.cols.max(cells.len());
                    self.rows.insert(row.row, cells.clone());
                    if changed {
                        out.push(WireUpdate::Row {
                            row: usize_to_u32(row.row),
                            seq: row.seq,
                            cells,
                        });
                    }
                }
                CacheUpdate::Rect(rect) => {
                    let mut changed = !dedupe;
                    let value: u64 = rect.cell.into();
                    self.cols = self.cols.max(rect.cols.end);
                    for r in rect.rows.clone() {
                        let row_vec = self.ensure_row_capacity(r, rect.cols.end);
                        for c in rect.cols.clone() {
                            if dedupe && !changed && row_vec[c] != value {
                                changed = true;
                            }
                            row_vec[c] = value;
                        }
                    }
                    if changed {
                        out.push(WireUpdate::Rect {
                            rows: [usize_to_u32(rect.rows.start), usize_to_u32(rect.rows.end)],
                            cols: [usize_to_u32(rect.cols.start), usize_to_u32(rect.cols.end)],
                            seq: rect.seq,
                            cell: value,
                        });
                    }
                }
                CacheUpdate::Cell(cell) => {
                    let value: u64 = cell.cell.into();
                    let row_vec = self.ensure_row_capacity(cell.row, cell.col + 1);
                    let previous = row_vec[cell.col];
                    row_vec[cell.col] = value;
                    if !dedupe || previous != value {
                        out.push(WireUpdate::Cell {
                            row: usize_to_u32(cell.row),
                            col: usize_to_u32(cell.col),
                            seq: cell.seq,
                            cell: value,
                        });
                    }
                }
                CacheUpdate::Trim(trim) => {
                    self.trim_rows(trim.start, trim.count);
                    trace!(
                        target = "sync::transmitter",
                        start = trim.start,
                        count = trim.count,
                        seq = trim.seq(),
                        marker = "tail_base_row_v3"
                    );
                    out.push(WireUpdate::Trim {
                        start: usize_to_u32(trim.start),
                        count: usize_to_u32(trim.count),
                        seq: trim.seq(),
                    });
                }
                CacheUpdate::Style(style) => {
                    let current = (style.style.fg, style.style.bg, style.style.attrs);
                    let prev = self.styles.insert(style.id.0, current);
                    if !dedupe || prev != Some(current) {
                        out.push(WireUpdate::Style {
                            id: style.id.0,
                            seq: style.seq,
                            fg: style.style.fg,
                            bg: style.style.bg,
                            attrs: style.style.attrs,
                        });
                    }
                }
                CacheUpdate::Cursor(cursor_state) => {
                    let candidate = CursorFrame {
                        row: usize_to_u32(cursor_state.row),
                        col: usize_to_u32(cursor_state.col),
                        seq: cursor_state.seq,
                        visible: cursor_state.visible,
                        blink: cursor_state.blink,
                    };
                    match next_cursor {
                        Some(ref existing) if existing.seq >= candidate.seq => {}
                        _ => next_cursor = Some(candidate),
                    }
                }
            }
        }
        let cursor = next_cursor.and_then(|candidate| {
            let emit = match self.cursor.as_ref() {
                Some(prev) => {
                    candidate.seq > prev.seq
                        || candidate.row != prev.row
                        || candidate.col != prev.col
                        || candidate.visible != prev.visible
                        || candidate.blink != prev.blink
                }
                None => true,
            };
            if emit {
                self.cursor = Some(candidate.clone());
                Some(candidate)
            } else {
                None
            }
        });

        PreparedUpdateBatch {
            updates: out,
            cursor,
        }
    }

    fn ensure_row_capacity(&mut self, row: usize, min_cols: usize) -> &mut Vec<u64> {
        let columns = self.cols.max(min_cols);
        let entry = self
            .rows
            .entry(row)
            .or_insert_with(|| vec![0; columns.max(1)]);
        if entry.len() < columns {
            entry.resize(columns, 0);
        }
        if entry.len() < min_cols {
            entry.resize(min_cols, 0);
        }
        entry
    }

    fn trim_rows(&mut self, start: usize, count: usize) {
        if count == 0 {
            return;
        }
        let end = start.saturating_add(count);
        self.rows.retain(|row, _| *row >= end);
    }
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[derive(Default)]
pub(crate) struct PreparedUpdateBatch {
    pub(crate) updates: Vec<WireUpdate>,
    pub(crate) cursor: Option<CursorFrame>,
}

impl TerminalDeltaStream for TimelineDeltaStream {
    fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate> {
        let history = self.history.lock().unwrap();
        history
            .iter()
            .filter(|update| update.seq() > since)
            .take(budget)
            .cloned()
            .collect()
    }

    fn latest_seq(&self) -> Seq {
        self.latest.load(Ordering::Relaxed)
    }
}

pub(crate) enum ForwarderCommand {
    AddTransport {
        transport: Arc<dyn Transport>,
        supervisor: Option<Arc<TransportSupervisor>>,
    },
    RemoveTransport {
        id: TransportId,
    },
    ViewportRefresh,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_update_forwarder(
    transports: Vec<ForwardTransport>,
    mut updates: UnboundedReceiver<CacheUpdate>,
    timeline: Arc<TimelineDeltaStream>,
    terminal_sync: Arc<TerminalSync>,
    sync_config: SyncConfig,
    mut backfill_rx: UnboundedReceiver<BackfillCommand>,
    mut command_rx: UnboundedReceiver<ForwarderCommand>,
    forwarder_tx: Option<UnboundedSender<ForwarderCommand>>,
    shared_registry: Arc<Mutex<Vec<Arc<SharedTransport>>>>,
    cursor_sync: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        struct Sink {
            transport: Arc<dyn Transport>,
            supervisor: Option<Arc<TransportSupervisor>>,
            synchronizer: ServerSynchronizer<TerminalSync, CacheUpdate>,
            last_seq: Seq,
            active: bool,
            handshake_complete: bool,
            last_handshake: Instant,
            handshake_attempts: u32,
            cache: TransmitterCache,
            backfill_queue: VecDeque<BackfillJob>,
            last_backfill_sent: Option<Instant>,
        }

        const HANDSHAKE_REFRESH: Duration = Duration::from_millis(200);

        let forwarder_tx = forwarder_tx;

        fn is_data_channel_not_open(err: &TransportError) -> bool {
            matches!(err, TransportError::Setup(message) if message.contains("DataChannel is not opened"))
        }

        fn drop_transport(
            sinks: &mut Vec<Sink>,
            shared_registry: &Arc<Mutex<Vec<Arc<SharedTransport>>>>,
            id: TransportId,
        ) {
            let before = sinks.len();
            sinks.retain(|sink| sink.transport.id() != id);
            if sinks.len() < before {
                info!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    removed = before - sinks.len(),
                    "removed transport sink"
                );
            } else {
                debug!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    "remove command ignored: transport not found"
                );
            }
            let mut registry = shared_registry.lock().unwrap();
            let registry_before = registry.len();
            registry.retain(|shared| shared.id() != id);
            if registry.len() < registry_before {
                trace!(
                    target = "sync::forwarder",
                    transport_id = id.0,
                    removed = registry_before - registry.len(),
                    "pruned shared transport registry"
                );
            }
        }

        fn request_transport_removal(
            id: TransportId,
            forwarder_tx: &Option<UnboundedSender<ForwarderCommand>>,
            sinks: &mut Vec<Sink>,
            shared_registry: &Arc<Mutex<Vec<Arc<SharedTransport>>>>,
        ) {
            let mut dispatched = false;
            if let Some(tx) = forwarder_tx {
                match tx.send(ForwarderCommand::RemoveTransport { id }) {
                    Ok(()) => {
                        dispatched = true;
                        trace!(
                            target = "sync::forwarder",
                            transport_id = id.0,
                            "enqueued transport removal command"
                        );
                    }
                    Err(_) => {
                        trace!(
                            target = "sync::forwarder",
                            transport_id = id.0,
                            "failed to enqueue transport removal; removing locally"
                        );
                    }
                }
            }
            if !dispatched {
                drop_transport(sinks, shared_registry, id);
            }
        }

        let subscription = SubscriptionId(1);
        let grid = terminal_sync.grid().clone();
        let mut next_backfill_index: usize = 0;
        let mut sinks: Vec<Sink> = transports
            .into_iter()
            .map(|(transport, supervisor)| Sink {
                synchronizer: ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone()),
                transport,
                supervisor,
                last_seq: 0,
                active: true,
                handshake_complete: false,
                last_handshake: Instant::now(),
                handshake_attempts: 0,
                cache: TransmitterCache::new(),
                backfill_queue: VecDeque::new(),
                last_backfill_sent: None,
            })
            .collect();

        let mut stale_transports: Vec<TransportId> = Vec::new();

        for sink in sinks.iter_mut() {
            match initialize_transport_snapshot(
                &sink.transport,
                subscription,
                &terminal_sync,
                &sync_config,
                &mut sink.cache,
                cursor_sync,
            ) {
                Ok((sync, seq)) => {
                    sink.synchronizer = sync;
                    sink.last_seq = seq;
                    sink.handshake_complete = true;
                }
                Err(err) => {
                    sink.handshake_complete = false;
                    let transport_id = sink.transport.id();
                    if is_data_channel_not_open(&err) {
                        sink.active = false;
                        sink.backfill_queue.clear();
                        stale_transports.push(transport_id);
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "initial handshake failed: data channel not open"
                        );
                    } else {
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "initial handshake failed"
                        );
                    }
                    if let Some(supervisor) = &sink.supervisor {
                        supervisor.schedule_reconnect();
                    }
                }
            }
            sink.last_handshake = Instant::now();
        }

        if !stale_transports.is_empty() {
            for id in stale_transports.drain(..) {
                request_transport_removal(id, &forwarder_tx, &mut sinks, &shared_registry);
            }
        }

        fn attempt_handshake(
            sink: &mut Sink,
            subscription: SubscriptionId,
            terminal_sync: &Arc<TerminalSync>,
            sync_config: &SyncConfig,
            stale_transports: &mut Vec<TransportId>,
            cursor_sync: bool,
        ) {
            sink.handshake_attempts = sink.handshake_attempts.saturating_add(1);
            debug!(
                target = "sync::handshake",
                transport_id = sink.transport.id().0,
                transport = ?sink.transport.kind(),
                attempt = sink.handshake_attempts,
                "starting handshake replay"
            );
            sink.last_handshake = Instant::now();
            match initialize_transport_snapshot(
                &sink.transport,
                subscription,
                terminal_sync,
                sync_config,
                &mut sink.cache,
                cursor_sync,
            ) {
                Ok((sync, seq)) => {
                    sink.synchronizer = sync;
                    sink.last_seq = seq;
                    sink.handshake_complete = true;
                    debug!(
                        target = "sync::handshake",
                        transport_id = sink.transport.id().0,
                        transport = ?sink.transport.kind(),
                        watermark = seq,
                        "handshake complete"
                    );
                }
                Err(err) => {
                    sink.handshake_complete = false;
                    let transport_id = sink.transport.id();
                    if is_data_channel_not_open(&err) {
                        sink.active = false;
                        sink.backfill_queue.clear();
                        stale_transports.push(transport_id);
                        warn!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "handshake attempt failed: data channel not open"
                        );
                    } else {
                        debug!(
                            target = "sync::handshake",
                            transport_id = transport_id.0,
                            transport = ?sink.transport.kind(),
                            error = %err,
                            "handshake attempt did not complete"
                        );
                    }
                    if let Some(supervisor) = &sink.supervisor {
                        supervisor.schedule_reconnect();
                    }
                }
            }
        }

        let mut handshake_timer = interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                _ = handshake_timer.tick() => {
                    for sink in sinks.iter_mut().filter(|s| s.active && !s.handshake_complete) {
                        if sink.last_handshake.elapsed() < HANDSHAKE_REFRESH {
                            continue;
                        }
                        attempt_handshake(
                            sink,
                            subscription,
                            &terminal_sync,
                            &sync_config,
                            &mut stale_transports,
                            cursor_sync,
                        );
                    }
                }
                maybe_update = updates.recv() => {
                    match maybe_update {
                        Some(update) => {
                            timeline.record(&update);
                            trace!(target = "sync::timeline", seq = update.seq(), "recorded cache update");

                            let mut drained = 1usize;
                            while let Ok(extra) = updates.try_recv() {
                                trace!(target = "sync::timeline", seq = extra.seq(), "recorded coalesced update");
                                timeline.record(&extra);
                                drained = drained.saturating_add(1);
                            }
                            telemetry::record_gauge("sync_updates_batch", drained as u64);

                            for sink in sinks.iter_mut().filter(|s| s.active && s.handshake_complete) {
                                let mut batches_sent = 0usize;
                                loop {
                                    let Some(batch) = sink.synchronizer.delta_batch(subscription, sink.last_seq) else { break; };
                                    if batch.updates.is_empty() {
                                        if batch.has_more {
                                            continue;
                                        }
                                        break;
                                    }
                                    telemetry::record_gauge("sync_delta_batch_updates", batch.updates.len() as u64);
                                    let converted_batch = sink.cache.apply_updates(&batch.updates, true);
                                    let _guard = PerfGuard::new("sync_send_delta");
                                    match send_delta_frames_chunked(
                                        &sink.transport,
                                        batch.subscription_id,
                                        batch.watermark.0,
                                        batch.has_more,
                                        converted_batch,
                                    ) {
                                        Ok(()) => {
                                            sink.last_seq = batch.watermark.0;
                                            sink.last_handshake = Instant::now();
                                            batches_sent = batches_sent.saturating_add(1);
                                        }
                                        Err(err) => {
                                            let transport_id = sink.transport.id();
                                            sink.handshake_complete = false;
                                            if is_data_channel_not_open(&err) {
                                                sink.active = false;
                                                sink.backfill_queue.clear();
                                                stale_transports.push(transport_id);
                                                warn!(
                                                    target = "sync::handshake",
                                                    transport_id = transport_id.0,
                                                    transport = ?sink.transport.kind(),
                                                    error = %err,
                                                    "delta send failed: data channel not open"
                                                );
                                            } else {
                                                warn!(
                                                    target = "sync::handshake",
                                                    transport_id = transport_id.0,
                                                    transport = ?sink.transport.kind(),
                                                    error = %err,
                                                    "delta send failed, marking handshake incomplete"
                                                );
                                            }
                                            if let Some(supervisor) = &sink.supervisor {
                                                supervisor.schedule_reconnect();
                                            }
                                            break;
                                        }
                                    }
                                    trace!(
                                        target = "sync::timeline",
                                        transport_id = sink.transport.id().0,
                                        transport = ?sink.transport.kind(),
                                        watermark = batch.watermark.0,
                                        updates = batch.updates.len(),
                                        has_more = batch.has_more,
                                        "delta batch delivered"
                                    );
                                    if !batch.has_more || batches_sent > 32 {
                                        break;
                                    }
                                }
                                telemetry::record_gauge("sync_delta_batches_sent", batches_sent as u64);
                            }
                        }
                        None => break,
                    }
                }
                maybe_forwarder = command_rx.recv() => {
                    if let Some(command) = maybe_forwarder {
                        match command {
                            ForwarderCommand::AddTransport { transport, supervisor } => {
                                let mut sink = Sink {
                                    synchronizer: ServerSynchronizer::new(
                                        terminal_sync.clone(),
                                        sync_config.clone(),
                                    ),
                                    transport: transport.clone(),
                                    supervisor,
                                    last_seq: 0,
                                    active: true,
                                    handshake_complete: false,
                                    last_handshake: Instant::now(),
                                    handshake_attempts: 0,
                                    cache: TransmitterCache::new(),
                                    backfill_queue: VecDeque::new(),
                                    last_backfill_sent: None,
                                };

                                match initialize_transport_snapshot(
                                    &sink.transport,
                                    subscription,
                                    &terminal_sync,
                                    &sync_config,
                                    &mut sink.cache,
                                    cursor_sync,
                                ) {
                                    Ok((sync, seq)) => {
                                        sink.synchronizer = sync;
                                        sink.last_seq = seq;
                                        sink.handshake_complete = true;
                                    }
                                    Err(err) => {
                                        sink.handshake_complete = false;
                                        let transport_id = sink.transport.id();
                                        if is_data_channel_not_open(&err) {
                                            sink.active = false;
                                            sink.backfill_queue.clear();
                                            stale_transports.push(transport_id);
                                            warn!(
                                                target = "sync::handshake",
                                                transport_id = transport_id.0,
                                                transport = ?sink.transport.kind(),
                                                error = %err,
                                                "handshake failed for new transport: data channel not open"
                                            );
                                        } else {
                                            warn!(
                                                target = "sync::handshake",
                                                transport_id = transport_id.0,
                                                transport = ?sink.transport.kind(),
                                                error = %err,
                                                "handshake failed for new transport"
                                            );
                                        }
                                        if let Some(supervisor) = &sink.supervisor {
                                            supervisor.schedule_reconnect();
                                        }
                                    }
                                }
                                sink.last_handshake = Instant::now();
                                sinks.push(sink);
                            }
                            ForwarderCommand::RemoveTransport { id } => {
                                drop_transport(&mut sinks, &shared_registry, id);
                            }
                            ForwarderCommand::ViewportRefresh => {
                                let (_, cols) = grid.viewport_size();
                                for sink in sinks.iter_mut() {
                                    if !sink.active {
                                        continue;
                                    }
                                    sink.synchronizer.reset();
                                    sink.cache.reset(cols);
                                    sink.handshake_complete = false;
                                    sink.handshake_attempts = 0;
                                    sink.last_handshake = Instant::now() - HANDSHAKE_REFRESH;
                                }
                            }
                        }
                    }
                }
                maybe_command = backfill_rx.recv() => {
                    if let Some(command) = maybe_command {
                        let end_row = command.start_row.saturating_add(command.count as u64);
                        if end_row <= command.start_row {
                            continue;
                        }
                        if let Some(sink) = sinks
                            .iter_mut()
                            .find(|s| s.transport.id() == command.transport_id)
                        {
                            sink.backfill_queue.push_back(BackfillJob {
                                subscription: command.subscription,
                                request_id: command.request_id,
                                next_row: command.start_row,
                                end_row,
                            });
                            trace!(
                                target = "sync::backfill",
                                transport_id = sink.transport.id().0,
                                request_id = command.request_id,
                                start_row = command.start_row,
                                count = command.count,
                                queued = sink.backfill_queue.len(),
                                "enqueued backfill request"
                            );
                        } else {
                            debug!(
                                target = "sync::backfill",
                                transport = command.transport_id.0,
                                "backfill request dropped: transport not found"
                            );
                        }
                    }
                }
            }

            let sink_count = sinks.len();
            if sink_count > 0 {
                if next_backfill_index >= sink_count {
                    next_backfill_index = 0;
                }
                for _ in 0..sink_count {
                    if sinks.is_empty() {
                        break;
                    }
                    if next_backfill_index >= sinks.len() {
                        next_backfill_index = 0;
                    }
                    let idx = next_backfill_index;
                    next_backfill_index = (next_backfill_index + 1) % sinks.len().max(1);
                    let sink = &mut sinks[idx];
                    if !sink.active || !sink.handshake_complete {
                        continue;
                    }
                    if sink.backfill_queue.is_empty() {
                        continue;
                    }
                    if let Some(last) = sink.last_backfill_sent {
                        if last.elapsed() < SERVER_BACKFILL_THROTTLE {
                            continue;
                        }
                    }
                    let mut job = match sink.backfill_queue.pop_front() {
                        Some(job) => job,
                        None => continue,
                    };
                    if job.next_row >= job.end_row {
                        continue;
                    }
                    let chunk_start = job.next_row;
                    let remaining = job.end_row.saturating_sub(chunk_start);
                    let chunk_rows = remaining
                        .min(MAX_BACKFILL_ROWS_PER_REQUEST as u64)
                        .min(SERVER_BACKFILL_CHUNK_ROWS as u64)
                        as u32;
                    let chunk = collect_backfill_chunk(&grid, chunk_start, chunk_rows);
                    let chunk_advance = chunk.attempted as u64;
                    let next_row = chunk_start.saturating_add(chunk_advance);
                    let more_pending = next_row < job.end_row;
                    let request_id = job.request_id;
                    let converted_batch = sink.cache.apply_updates(&chunk.updates, false);
                    match send_host_frame(
                        &sink.transport,
                        HostFrame::HistoryBackfill {
                            subscription: job.subscription,
                            request_id: job.request_id,
                            start_row: chunk_start,
                            count: chunk.attempted,
                            updates: converted_batch.updates,
                            more: more_pending,
                            cursor: converted_batch.cursor,
                        },
                    ) {
                        Ok(()) => {
                            sink.last_backfill_sent = Some(Instant::now());
                            job.next_row = next_row;
                            if more_pending {
                                sink.backfill_queue.push_back(job);
                            }
                            trace!(
                                target = "sync::backfill",
                                transport_id = sink.transport.id().0,
                                request_id,
                                start_row = chunk_start,
                                count = chunk.attempted,
                                delivered = chunk.delivered,
                                more = more_pending,
                                "sent backfill chunk"
                            );
                        }
                        Err(err) => {
                            let transport_id = sink.transport.id();
                            sink.handshake_complete = false;
                            if is_data_channel_not_open(&err) {
                                sink.active = false;
                                sink.backfill_queue.clear();
                                stale_transports.push(transport_id);
                                warn!(
                                    target = "sync::backfill",
                                    transport_id = transport_id.0,
                                    transport = ?sink.transport.kind(),
                                    error = %err,
                                    "backfill send failed: data channel not open"
                                );
                            } else {
                                sink.backfill_queue.push_front(job);
                                warn!(
                                    target = "sync::backfill",
                                    transport_id = transport_id.0,
                                    transport = ?sink.transport.kind(),
                                    error = %err,
                                    "backfill send failed; scheduling reconnect"
                                );
                            }
                            if let Some(supervisor) = &sink.supervisor {
                                supervisor.schedule_reconnect();
                            }
                        }
                    }
                    break;
                }
            }

            if !stale_transports.is_empty() {
                for id in stale_transports.drain(..) {
                    request_transport_removal(id, &forwarder_tx, &mut sinks, &shared_registry);
                }
            }
        }
    })
}

pub(crate) fn initialize_transport_snapshot(
    transport: &Arc<dyn Transport>,
    subscription: SubscriptionId,
    terminal_sync: &Arc<TerminalSync>,
    sync_config: &SyncConfig,
    cache: &mut TransmitterCache,
    cursor_sync: bool,
) -> Result<(ServerSynchronizer<TerminalSync, CacheUpdate>, Seq), TransportError> {
    let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
    let hello = synchronizer.hello(subscription);
    let features = if cursor_sync { FEATURE_CURSOR_SYNC } else { 0 };
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        "sending server hello"
    );
    send_host_frame(
        transport,
        HostFrame::Hello {
            subscription: hello.subscription_id.0,
            max_seq: hello.max_seq.0,
            config: sync_config_to_wire(&hello.config),
            features,
        },
    )?;
    let (viewport_rows, cols) = terminal_sync.grid().viewport_size();
    let history_rows = terminal_sync.grid().rows();
    cache.reset(cols);
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        viewport_rows,
        cols,
        history_rows,
        "sending grid descriptor"
    );
    send_host_frame(
        transport,
        HostFrame::Grid {
            cols: cols as u32,
            history_rows: history_rows as u32,
            base_row: terminal_sync.grid().row_offset(),
            viewport_rows: None,
        },
    )?;
    transmit_initial_snapshots(transport, &mut synchronizer, cache, subscription)?;
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        watermark = hello.max_seq.0,
        "initial snapshots transmitted"
    );
    debug!(
        target = "sync::handshake",
        transport_id = transport.id().0,
        transport = ?transport.kind(),
        lanes = 3usize,
        watermark = hello.max_seq.0,
        "initial snapshots complete"
    );
    Ok((synchronizer, hello.max_seq.0))
}

pub(crate) fn sync_config_to_wire(config: &SyncConfig) -> WireSyncConfig {
    let snapshot_budgets = config
        .snapshot_budgets
        .iter()
        .map(|LaneBudget { lane, max_updates }| WireLaneBudget {
            lane: map_lane(*lane),
            max_updates: *max_updates as u32,
        })
        .collect();

    WireSyncConfig {
        snapshot_budgets,
        delta_budget: config.delta_budget as u32,
        heartbeat_ms: config.heartbeat_interval.as_millis() as u64,
        initial_snapshot_lines: config.initial_snapshot_lines as u32,
    }
}

pub(crate) fn transmit_initial_snapshots(
    transport: &Arc<dyn Transport>,
    synchronizer: &mut ServerSynchronizer<TerminalSync, CacheUpdate>,
    cache: &mut TransmitterCache,
    subscription: SubscriptionId,
) -> Result<(), TransportError> {
    let transport_id = transport.id().0;
    let transport_kind = transport.kind();
    for lane in [
        PriorityLane::Foreground,
        PriorityLane::Recent,
        PriorityLane::History,
    ] {
        let mut emitted_chunk = false;
        while let Some(chunk) = synchronizer.snapshot_chunk(subscription, lane) {
            emitted_chunk = true;
            debug!(
                target = "sync::handshake",
                transport_id,
                transport = ?transport_kind,
                lane = ?lane,
                updates = chunk.updates.len(),
                "sending snapshot chunk"
            );
            let converted_batch = cache.apply_updates(&chunk.updates, false);
            send_snapshot_frames_chunked(
                transport,
                chunk.subscription_id,
                lane,
                chunk.watermark.0,
                chunk.has_more,
                converted_batch,
            )?;
            if !chunk.has_more {
                debug!(
                    target = "sync::handshake",
                    transport_id,
                    transport = ?transport_kind,
                    lane = ?lane,
                    "lane snapshot complete"
                );
                send_host_frame(
                    transport,
                    HostFrame::SnapshotComplete {
                        subscription: subscription.0,
                        lane: map_lane(lane),
                    },
                )?;
            }
        }
        if !emitted_chunk {
            debug!(
                target = "sync::handshake",
                transport_id,
                transport = ?transport_kind,
                lane = ?lane,
                "lane snapshot empty; sending completion"
            );
            send_host_frame(
                transport,
                HostFrame::SnapshotComplete {
                    subscription: subscription.0,
                    lane: map_lane(lane),
                },
            )?;
        }
    }
    Ok(())
}

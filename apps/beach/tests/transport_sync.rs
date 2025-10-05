use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use beach_human::cache::terminal::{PackedCell, Style, StyleId, TerminalGrid};
use beach_human::cache::{GridCache, Seq, WriteOutcome};
use beach_human::model::terminal::diff::{CacheUpdate, CellWrite, RectFill};
use beach_human::protocol::{
    self, CursorFrame, HostFrame, Lane as WireLane, LaneBudgetFrame as WireLaneBudget,
    SyncConfigFrame as WireSyncConfig, Update as WireUpdate,
};
use beach_human::sync::terminal::sync::{TerminalDeltaStream, TerminalSync};
use beach_human::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use beach_human::transport::{
    IpcBuilder, Payload, Transport, TransportBuilder, TransportKind, TransportMessage,
    WebRtcBuilder, WebSocketBuilder,
};

struct BufferedDeltaStream {
    updates: Mutex<Vec<CacheUpdate>>,
    latest: AtomicU64,
}

impl BufferedDeltaStream {
    fn new() -> Self {
        Self {
            updates: Mutex::new(Vec::new()),
            latest: AtomicU64::new(0),
        }
    }

    fn push(&self, update: CacheUpdate) {
        self.latest.store(update.seq(), Ordering::Relaxed);
        self.updates.lock().unwrap().push(update);
    }
}

impl TerminalDeltaStream for BufferedDeltaStream {
    fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate> {
        let updates = self.updates.lock().unwrap();
        updates
            .iter()
            .filter(|u| u.seq() > since)
            .take(budget)
            .cloned()
            .collect()
    }

    fn latest_seq(&self) -> Seq {
        self.latest.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct TransmitterCache {
    cols: usize,
    rows: HashMap<usize, Vec<u64>>,
    styles: HashMap<u32, (u32, u32, u8)>,
    cursor: Option<CursorFrame>,
}

impl TransmitterCache {
    fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self, cols: usize) {
        self.cols = cols;
        self.rows.clear();
        self.styles.clear();
        self.cursor = None;
    }

    fn apply_updates(&mut self, updates: &[CacheUpdate], dedupe: bool) -> PreparedUpdateBatch {
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
                    next_cursor = Some(CursorFrame {
                        row: usize_to_u32(cursor_state.row),
                        col: usize_to_u32(cursor_state.col),
                        seq: cursor_state.seq,
                        visible: cursor_state.visible,
                        blink: cursor_state.blink,
                    });
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
struct PreparedUpdateBatch {
    updates: Vec<WireUpdate>,
    cursor: Option<CursorFrame>,
}

fn map_lane(lane: PriorityLane) -> WireLane {
    match lane {
        PriorityLane::Foreground => WireLane::Foreground,
        PriorityLane::Recent => WireLane::Recent,
        PriorityLane::History => WireLane::History,
    }
}

fn sync_config_to_wire(config: &SyncConfig) -> WireSyncConfig {
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

fn apply_wire_update(update: &WireUpdate, grid: &TerminalGrid) {
    match update {
        WireUpdate::Row { row, seq, cells } => {
            let row = *row as usize;
            for (col, raw) in cells.iter().enumerate() {
                let packed = PackedCell::from_raw(*raw);
                let _ = grid.write_packed_cell_if_newer(row, col, *seq, packed);
            }
        }
        WireUpdate::RowSegment {
            row,
            start_col,
            seq,
            cells,
        } => {
            let row = *row as usize;
            for (idx, raw) in cells.iter().enumerate() {
                let col = *start_col as usize + idx;
                let packed = PackedCell::from_raw(*raw);
                let _ = grid.write_packed_cell_if_newer(row, col, *seq, packed);
            }
        }
        WireUpdate::Rect {
            rows,
            cols,
            seq,
            cell,
        } => {
            let packed = PackedCell::from_raw(*cell);
            let _ = grid.fill_rect_with_cell_if_newer(
                rows[0] as usize,
                cols[0] as usize,
                rows[1] as usize,
                cols[1] as usize,
                *seq,
                packed,
            );
        }
        WireUpdate::Cell {
            row,
            col,
            seq,
            cell,
        } => {
            let packed = PackedCell::from_raw(*cell);
            let _ = grid.write_packed_cell_if_newer(*row as usize, *col as usize, *seq, packed);
        }
        WireUpdate::Trim { .. } => {
            // trimming is applied via client-side renderer; cache grid ignores
        }
        WireUpdate::Style {
            id,
            seq: _,
            fg,
            bg,
            attrs,
        } => {
            let _ = grid.style_table.set(
                StyleId(*id),
                Style {
                    fg: *fg,
                    bg: *bg,
                    attrs: *attrs,
                },
            );
        }
    }
}

fn apply_cache_update(update: &CacheUpdate, grid: &TerminalGrid) {
    match update {
        CacheUpdate::Row(row) => {
            for (col, cell) in row.cells.iter().enumerate() {
                let _ = grid.write_packed_cell_if_newer(row.row, col, row.seq, *cell);
            }
        }
        CacheUpdate::Rect(rect) => {
            let _ = grid.fill_rect_with_cell_if_newer(
                rect.rows.start,
                rect.cols.start,
                rect.rows.end,
                rect.cols.end,
                rect.seq,
                rect.cell,
            );
        }
        CacheUpdate::Cell(cell) => {
            let _ = grid.write_packed_cell_if_newer(cell.row, cell.col, cell.seq, cell.cell);
        }
        CacheUpdate::Trim(_) => {}
        CacheUpdate::Style(style) => {
            let _ = grid.style_table.set(style.id, style.style);
        }
        CacheUpdate::Cursor(_) => {}
    }
}

fn send_host_frame(transport: &dyn Transport, frame: HostFrame) {
    let bytes = protocol::encode_host_frame_binary(&frame);
    transport.send_bytes(&bytes).expect("transport send");
}

fn recv_host_frame(transport: &dyn Transport) -> HostFrame {
    loop {
        match transport.recv(Duration::from_secs(5)) {
            Ok(TransportMessage {
                payload: Payload::Binary(bytes),
                ..
            }) => {
                return protocol::decode_host_frame_binary(&bytes).expect("host frame");
            }
            Ok(TransportMessage {
                payload: Payload::Text(text),
                ..
            }) => {
                let trimmed = text.trim();
                if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                    continue;
                }
            }
            Err(err) => panic!("transport recv failed: {err:?}"),
        }
    }
}

fn fill_row(grid: &TerminalGrid, row: usize, text: &str, seq_base: Seq) {
    for (offset, ch) in text.chars().enumerate() {
        let packed = TerminalGrid::pack_char_with_style(ch, StyleId::DEFAULT);
        let seq = seq_base + offset as Seq;
        let outcome = grid.write_packed_cell_if_newer(row, offset, seq, packed);
        assert!(matches!(
            outcome,
            Ok(WriteOutcome::Written | WriteOutcome::SkippedEqual)
        ));
    }
}

fn assert_grids_match(server: &TerminalGrid, client: &TerminalGrid) {
    let (rows, cols) = server.dims();
    for row in 0..rows {
        for col in 0..cols {
            let server_cell = server.get_cell_relaxed(row, col).expect("server cell");
            let client_cell = client.get_cell_relaxed(row, col).expect("client cell");
            assert_eq!(
                server_cell.cell.into_raw(),
                client_cell.cell.into_raw(),
                "cell mismatch at row {row} col {col}"
            );
        }
    }
}

fn run_transport_integration<B>(builder: B, _kind: TransportKind)
where
    B: TransportBuilder,
{
    let pair = builder.build_pair().expect("build transport pair");
    let server_transport = pair.server;
    let client_transport = pair.client;

    let rows: usize = 20;
    let cols: usize = 6;
    let server_grid = Arc::new(TerminalGrid::new(rows, cols));
    let client_grid = Arc::new(TerminalGrid::new(rows, cols));

    // Baseline content
    for row in 0..rows {
        let text = format!("row-{row:02}");
        fill_row(&server_grid, row, &text, (row as Seq) * 100);
    }

    // Prepare delta updates to be applied after snapshots
    let style_id = StyleId::DEFAULT;
    let delta_updates = vec![
        CacheUpdate::Cell(CellWrite::new(
            rows - 1,
            0,
            10_000,
            TerminalGrid::pack_char_with_style('Δ', style_id),
        )),
        CacheUpdate::Rect(RectFill::new(
            rows - 3..rows - 1,
            1..3,
            10_010,
            TerminalGrid::pack_char_with_style('*', style_id),
        )),
    ];

    let delta_stream = Arc::new(BufferedDeltaStream::new());

    let config = SyncConfig {
        snapshot_budgets: vec![
            LaneBudget::new(PriorityLane::Foreground, 4),
            LaneBudget::new(PriorityLane::Recent, 6),
            LaneBudget::new(PriorityLane::History, 8),
        ],
        delta_budget: 8,
        heartbeat_interval: Duration::from_millis(50),
        initial_snapshot_lines: 4,
    };

    let terminal_sync = Arc::new(TerminalSync::new(
        server_grid.clone(),
        delta_stream.clone(),
        config.clone(),
    ));
    let subscription_id = SubscriptionId(42);

    let barrier = Arc::new(Barrier::new(2));

    let server_barrier = barrier.clone();
    let server_grid_clone = server_grid.clone();
    let client_grid_clone = client_grid.clone();
    let delta_updates_clone = delta_updates.clone();
    let delta_stream_clone = delta_stream.clone();

    let server_handle = thread::spawn(move || {
        server_barrier.wait();
        let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), config.clone());
        let hello = synchronizer.hello(subscription_id);
        send_host_frame(
            server_transport.as_ref(),
            HostFrame::Hello {
                subscription: hello.subscription_id.0,
                max_seq: hello.max_seq.0,
                config: sync_config_to_wire(&hello.config),
                features: 0,
            },
        );
        send_host_frame(
            server_transport.as_ref(),
            HostFrame::Grid {
                cols: cols as u32,
                history_rows: rows as u32,
                base_row: server_grid_clone.row_offset(),
                viewport_rows: None,
            },
        );

        let mut tx_cache = TransmitterCache::new();
        tx_cache.reset(cols);

        for lane in [
            PriorityLane::Foreground,
            PriorityLane::Recent,
            PriorityLane::History,
        ] {
            while let Some(chunk) = synchronizer.snapshot_chunk(subscription_id, lane) {
                let converted_batch = tx_cache.apply_updates(&chunk.updates, false);
                send_host_frame(
                    server_transport.as_ref(),
                    HostFrame::Snapshot {
                        subscription: chunk.subscription_id.0,
                        lane: map_lane(lane),
                        watermark: chunk.watermark.0,
                        has_more: chunk.has_more,
                        updates: converted_batch.updates,
                        cursor: converted_batch.cursor,
                    },
                );
                if !chunk.has_more {
                    send_host_frame(
                        server_transport.as_ref(),
                        HostFrame::SnapshotComplete {
                            subscription: subscription_id.0,
                            lane: map_lane(lane),
                        },
                    );
                    break;
                }
            }
        }

        // Apply delta updates to the server grid (simulate live edits)
        for update in delta_updates_clone.iter() {
            delta_stream_clone.push(update.clone());
            apply_cache_update(update, &server_grid_clone);
        }

        let mut watermark = hello.max_seq.0;
        loop {
            let Some(batch) = synchronizer.delta_batch(subscription_id, watermark) else {
                break;
            };
            let converted_batch = tx_cache.apply_updates(&batch.updates, true);
            if converted_batch.updates.is_empty() {
                watermark = batch.watermark.0;
                if !batch.has_more {
                    break;
                }
                continue;
            }
            send_host_frame(
                server_transport.as_ref(),
                HostFrame::Delta {
                    subscription: batch.subscription_id.0,
                    watermark: batch.watermark.0,
                    has_more: batch.has_more,
                    updates: converted_batch.updates,
                    cursor: converted_batch.cursor,
                },
            );
            watermark = batch.watermark.0;
            if !batch.has_more {
                break;
            }
        }

        send_host_frame(server_transport.as_ref(), HostFrame::Shutdown);
    });

    let client_barrier = barrier.clone();
    let client_handle = thread::spawn(move || {
        client_barrier.wait();
        loop {
            let frame = recv_host_frame(client_transport.as_ref());
            match frame {
                HostFrame::Snapshot { updates, .. }
                | HostFrame::Delta { updates, .. }
                | HostFrame::HistoryBackfill { updates, .. } => {
                    for update in &updates {
                        apply_wire_update(update, &client_grid_clone);
                    }
                }
                HostFrame::Cursor { .. } => {}
                HostFrame::Shutdown => break,
                HostFrame::SnapshotComplete { .. }
                | HostFrame::Hello { .. }
                | HostFrame::Grid { .. }
                | HostFrame::Heartbeat { .. }
                | HostFrame::InputAck { .. } => {}
            }
        }
    });

    server_handle.join().expect("server thread");
    client_handle.join().expect("client thread");

    assert_grids_match(&server_grid, &client_grid);
}

#[test_timeout::timeout]
fn terminal_sync_over_all_transports() {
    run_transport_integration(WebRtcBuilder, TransportKind::WebRtc);
    run_transport_integration(WebSocketBuilder, TransportKind::WebSocket);
    run_transport_integration(IpcBuilder, TransportKind::Ipc);
}

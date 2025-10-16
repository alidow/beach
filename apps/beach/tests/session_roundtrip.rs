use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Duration as StdDuration, Instant};

use beach::cache::GridCache;
use beach::cache::terminal::{self, Style, StyleId, TerminalGrid};
use beach::model::terminal::diff::{CacheUpdate, RowSnapshot};
use beach::protocol::{
    self, ClientFrame as WireClientFrame, CursorFrame, HostFrame, Lane as WireLane,
    LaneBudgetFrame as WireLaneBudget, SyncConfigFrame as WireSyncConfig, Update as WireUpdate,
};
use beach::sync::terminal::{TerminalDeltaStream, TerminalSync};
use beach::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use beach::transport::{
    Payload, Transport, TransportError, TransportKind, TransportMessage, TransportPair,
};
use tokio::time::{Instant as TokioInstant, sleep};

#[test_timeout::timeout]
fn late_joiner_receives_snapshot_and_roundtrips_input() {
    let rows = 20;
    let cols = 32;

    let pair = TransportPair::new(TransportKind::Ipc);
    let host_transport: Arc<dyn Transport> = Arc::from(pair.server);
    let client_transport = pair.client;

    let grid = Arc::new(TerminalGrid::new(rows, cols));
    let style_id = grid.ensure_style_id(Style::default());

    let delta_stream = Arc::new(BufferedDeltaStream::new());
    let sync_config = SyncConfig::default();
    let terminal_sync = Arc::new(TerminalSync::new(
        grid.clone(),
        delta_stream.clone(),
        sync_config.clone(),
    ));

    let mut seq: u64 = 0;
    apply_row(
        &grid,
        style_id,
        15,
        &mut seq,
        "host% echo hello",
        delta_stream.as_ref(),
    );
    apply_row(
        &grid,
        style_id,
        16,
        &mut seq,
        "hello",
        delta_stream.as_ref(),
    );
    apply_row(
        &grid,
        style_id,
        17,
        &mut seq,
        "host% ",
        delta_stream.as_ref(),
    );

    let subscription = SubscriptionId(1);
    let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), sync_config.clone());
    let hello = synchronizer.hello(subscription);
    let mut tx_cache = TransmitterCache::new();
    tx_cache.reset(cols);

    send_host_frame(
        host_transport.as_ref(),
        HostFrame::Hello {
            subscription: subscription.0,
            max_seq: hello.max_seq.0,
            config: sync_config_to_wire(&hello.config),
            features: 0,
        },
    );
    send_host_frame(
        host_transport.as_ref(),
        HostFrame::Grid {
            cols: cols as u32,
            history_rows: rows as u32,
            base_row: grid.row_offset(),
            viewport_rows: None,
        },
    );

    for lane in [
        PriorityLane::Foreground,
        PriorityLane::Recent,
        PriorityLane::History,
    ] {
        let mut emitted_chunk = false;
        while let Some(chunk) = synchronizer.snapshot_chunk(subscription, lane) {
            emitted_chunk = true;
            let converted_batch = tx_cache.apply_updates(&chunk.updates, false);
            send_host_frame(
                host_transport.as_ref(),
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
                    host_transport.as_ref(),
                    HostFrame::SnapshotComplete {
                        subscription: subscription.0,
                        lane: map_lane(lane),
                    },
                );
                break;
            }
        }
        if !emitted_chunk {
            send_host_frame(
                host_transport.as_ref(),
                HostFrame::SnapshotComplete {
                    subscription: subscription.0,
                    lane: map_lane(lane),
                },
            );
        }
    }

    let mut client_view: Option<ClientGrid> = None;
    let mut history_complete = false;

    while !history_complete {
        let frame = recv_host_frame(client_transport.as_ref(), Duration::from_secs(1));
        match frame {
            HostFrame::Hello { .. } => {}
            HostFrame::Grid {
                cols,
                history_rows,
                base_row: _,
                viewport_rows: _,
            } => {
                client_view = Some(ClientGrid::new(history_rows as usize, cols as usize));
            }
            HostFrame::Snapshot { updates, lane, .. } => {
                let view = client_view.as_mut().expect("grid message before snapshot");
                for update in &updates {
                    view.apply_update(update);
                }
                if lane == WireLane::History {
                    // wait for completion frame
                }
            }
            HostFrame::SnapshotComplete { lane, .. } => {
                if lane == WireLane::History {
                    history_complete = true;
                }
            }
            other => panic!("unexpected snapshot message: {other:?}"),
        }
    }

    let view = client_view.expect("client view populated");
    assert!(view.contains_row("host% echo hello"));
    assert!(view.contains_row("hello"));
    assert!(view.contains_row("host% "));

    let input_bytes = b"echo world\r";
    let client_input = WireClientFrame::Input {
        seq: 1,
        data: input_bytes.to_vec(),
    };
    client_transport
        .send(TransportMessage::binary(
            0,
            protocol::encode_client_frame_binary(&client_input),
        ))
        .expect("send input");

    let inbound = host_transport
        .recv(Duration::from_secs(1))
        .expect("receive input frame");
    let ack_seq = match inbound.payload {
        Payload::Binary(bytes) => match protocol::decode_client_frame_binary(&bytes) {
            Ok(WireClientFrame::Input { seq, data }) => {
                assert_eq!(data.as_slice(), input_bytes);
                seq
            }
            other => panic!("unexpected client frame: {other:?}"),
        },
        Payload::Text(text) => panic!("unexpected text payload: {text}"),
    };

    apply_row(
        &grid,
        style_id,
        17,
        &mut seq,
        "host% echo world",
        delta_stream.as_ref(),
    );
    apply_row(
        &grid,
        style_id,
        18,
        &mut seq,
        "world",
        delta_stream.as_ref(),
    );
    apply_row(
        &grid,
        style_id,
        19,
        &mut seq,
        "host% ",
        delta_stream.as_ref(),
    );

    send_host_frame(
        host_transport.as_ref(),
        HostFrame::InputAck { seq: ack_seq },
    );

    let mut last_seq = hello.max_seq.0;
    while let Some(batch) = synchronizer.delta_batch(subscription, last_seq) {
        if batch.updates.is_empty() {
            if batch.has_more {
                last_seq = batch.watermark.0;
                continue;
            }
            break;
        }
        let converted_batch = tx_cache.apply_updates(&batch.updates, true);
        if converted_batch.updates.is_empty() {
            last_seq = batch.watermark.0;
            if !batch.has_more {
                break;
            }
            continue;
        }
        send_host_frame(
            host_transport.as_ref(),
            HostFrame::Delta {
                subscription: batch.subscription_id.0,
                watermark: batch.watermark.0,
                has_more: batch.has_more,
                updates: converted_batch.updates,
                cursor: converted_batch.cursor,
            },
        );
        last_seq = batch.watermark.0;
        if !batch.has_more {
            break;
        }
    }

    let mut view = view;
    let mut saw_ack = false;
    let mut saw_world = false;
    for _ in 0..6 {
        let frame = recv_host_frame(client_transport.as_ref(), Duration::from_secs(1));
        match frame {
            HostFrame::InputAck { .. } => saw_ack = true,
            HostFrame::Delta { updates, .. } => {
                for update in &updates {
                    view.apply_update(update);
                }
            }
            HostFrame::Heartbeat { .. }
            | HostFrame::Hello { .. }
            | HostFrame::Grid { .. }
            | HostFrame::Snapshot { .. }
            | HostFrame::SnapshotComplete { .. }
            | HostFrame::HistoryBackfill { .. }
            | HostFrame::Cursor { .. }
            | HostFrame::Shutdown => {}
        }
        if view.contains_row("host% echo world") && view.contains_row("world") {
            saw_world = true;
        }
        if saw_ack && saw_world {
            break;
        }
    }

    assert!(saw_ack, "client should receive input ack");
    assert!(saw_world, "client should render new command and output");
    let row_command = row_string(&grid, 17);
    assert_eq!(row_command.trim_end_matches(' '), "host% echo world");
    let row_output = row_string(&grid, 18);
    assert_eq!(row_output.trim_end_matches(' '), "world");
    let row_prompt = row_string(&grid, 19);
    assert!(row_prompt.starts_with("host% "));
}

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
    fn collect_since(&self, since: u64, budget: usize) -> Vec<CacheUpdate> {
        let updates = self.updates.lock().unwrap();
        updates
            .iter()
            .filter(|update| update.seq() > since)
            .take(budget)
            .cloned()
            .collect()
    }

    fn latest_seq(&self) -> u64 {
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

struct ClientGrid {
    rows: usize,
    cols: usize,
    cells: Vec<Vec<char>>,
}

impl ClientGrid {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            cells: vec![vec![' '; cols]; rows],
        }
    }

    fn apply_update(&mut self, update: &WireUpdate) {
        match update {
            WireUpdate::Row { row, cells, .. } => {
                let row = *row as usize;
                if row >= self.rows {
                    return;
                }
                for (col, cell) in cells.iter().enumerate().take(self.cols) {
                    self.cells[row][col] = decode_cell(*cell);
                }
            }
            WireUpdate::Cell { row, col, cell, .. } => {
                let row = *row as usize;
                let col = *col as usize;
                if row < self.rows && col < self.cols {
                    self.cells[row][col] = decode_cell(*cell);
                }
            }
            WireUpdate::Rect {
                rows, cols, cell, ..
            } => {
                let row0 = rows[0] as usize;
                let row1 = rows[1] as usize;
                let col0 = cols[0] as usize;
                let col1 = cols[1] as usize;
                let ch = decode_cell(*cell);
                for row in row0..row1.min(self.rows) {
                    for col in col0..col1.min(self.cols) {
                        self.cells[row][col] = ch;
                    }
                }
            }
            WireUpdate::RowSegment {
                row,
                start_col,
                cells,
                ..
            } => {
                let row = *row as usize;
                if row >= self.rows {
                    return;
                }
                for (idx, cell) in cells.iter().enumerate() {
                    let col = *start_col as usize + idx;
                    if col < self.cols {
                        self.cells[row][col] = decode_cell(*cell);
                    }
                }
            }
            WireUpdate::Trim { .. } | WireUpdate::Style { .. } => {}
        }
    }

    fn contains_row(&self, needle: &str) -> bool {
        self.cells.iter().any(|row| {
            let mut needle_chars: Vec<char> = needle.chars().collect();
            if matches!(needle_chars.last(), Some(' ')) {
                while matches!(needle_chars.last(), Some(' ')) {
                    needle_chars.pop();
                }
                let prefix_len = needle_chars.len();
                let prefix_matches = row
                    .iter()
                    .take(prefix_len)
                    .copied()
                    .eq(needle_chars.iter().copied());
                let suffix_blank = row.iter().skip(prefix_len).all(|&ch| ch == ' ');
                prefix_matches && suffix_blank
            } else {
                let text: String = row.iter().collect();
                text.trim_end_matches(' ') == needle
            }
        })
    }
}

fn apply_row(
    grid: &TerminalGrid,
    style: StyleId,
    row: usize,
    seq: &mut u64,
    text: &str,
    deltas: &BufferedDeltaStream,
) {
    *seq += 1;
    let seq_value = *seq;
    let (_, cols) = grid.dims();
    let mut packed = Vec::with_capacity(cols);
    let mut chars = text.chars();
    for col in 0..cols {
        let ch = chars.next().unwrap_or(' ');
        let cell = TerminalGrid::pack_char_with_style(ch, style);
        grid.write_packed_cell_if_newer(row, col, seq_value, cell)
            .unwrap();
        packed.push(cell);
    }
    let update = CacheUpdate::Row(RowSnapshot::new(row, seq_value, packed));
    deltas.push(update);
}

fn row_string(grid: &TerminalGrid, row: usize) -> String {
    let (_, cols) = grid.dims();
    let mut raw = vec![0u64; cols];
    grid.snapshot_row_into(row, &mut raw).unwrap();
    let mut text = String::with_capacity(cols);
    for payload in raw {
        let cell = terminal::PackedCell::from(payload);
        let (ch, _) = terminal::unpack_cell(cell);
        text.push(ch);
    }
    text
}

fn send_host_frame(transport: &dyn Transport, frame: HostFrame) {
    let bytes = protocol::encode_host_frame_binary(&frame);
    transport.send_bytes(&bytes).expect("send frame");
}

fn recv_host_frame(transport: &dyn Transport, timeout: StdDuration) -> HostFrame {
    let deadline = Instant::now() + timeout;
    loop {
        match transport.recv(timeout) {
            Ok(message) => match message.payload {
                Payload::Binary(bytes) => {
                    return protocol::decode_host_frame_binary(&bytes).expect("host frame");
                }
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                        continue;
                    }
                }
            },
            Err(TransportError::Timeout) => {
                if Instant::now() >= deadline {
                    panic!("timed out waiting for frame");
                }
                continue;
            }
            Err(TransportError::ChannelClosed) => panic!("transport channel closed"),
            Err(err) => panic!("transport error: {err}"),
        }
    }
}

#[allow(dead_code)]
async fn recv_host_frame_async(transport: &Arc<dyn Transport>, timeout: StdDuration) -> HostFrame {
    let deadline = TokioInstant::now() + timeout;
    loop {
        match transport.try_recv() {
            Ok(Some(message)) => match message.payload {
                Payload::Binary(bytes) => {
                    return protocol::decode_host_frame_binary(&bytes).expect("host frame");
                }
                Payload::Text(text) => {
                    let trimmed = text.trim();
                    if trimmed == "__ready__" || trimmed == "__offer_ready__" {
                        continue;
                    }
                }
            },
            Ok(None) => {}
            Err(TransportError::ChannelClosed) => panic!("transport channel closed"),
            Err(err) => panic!("transport error: {err}"),
        }
        if TokioInstant::now() >= deadline {
            panic!("timed out waiting for frame");
        }
        sleep(StdDuration::from_millis(10)).await;
    }
}

fn decode_cell(cell: u64) -> char {
    let packed = terminal::PackedCell::from(cell);
    terminal::unpack_cell(packed).0
}

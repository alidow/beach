use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use bincode;
use serde::{Deserialize, Serialize};

use beach_human::cache::terminal::{PackedCell, Style, StyleId, TerminalGrid};
use beach_human::cache::{GridCache, Seq, WriteOutcome};
use beach_human::model::terminal::diff::{CacheUpdate, CellWrite, RectFill};
use beach_human::sync::terminal::sync::{TerminalDeltaStream, TerminalSync};
use beach_human::sync::{LaneBudget, PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use beach_human::transport::{
    IpcBuilder, Payload, Transport, TransportBuilder, TransportKind, TransportMessage,
    WebRtcBuilder, WebSocketBuilder,
};

struct TestDeltaStream {
    updates: Vec<CacheUpdate>,
    latest_seq: Seq,
}

impl TestDeltaStream {
    fn new(updates: Vec<CacheUpdate>) -> Self {
        let latest_seq = updates.iter().map(|u| u.seq()).max().unwrap_or(0);
        Self {
            updates,
            latest_seq,
        }
    }
}

impl TerminalDeltaStream for TestDeltaStream {
    fn collect_since(&self, since: Seq, budget: usize) -> Vec<CacheUpdate> {
        self.updates
            .iter()
            .filter(|u| u.seq() > since)
            .take(budget)
            .cloned()
            .collect()
    }

    fn latest_seq(&self) -> Seq {
        self.latest_seq
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Lane {
    Foreground,
    Recent,
    History,
}

impl From<PriorityLane> for Lane {
    fn from(value: PriorityLane) -> Self {
        match value {
            PriorityLane::Foreground => Lane::Foreground,
            PriorityLane::Recent => Lane::Recent,
            PriorityLane::History => Lane::History,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RowUpdate {
    row: usize,
    seq: Seq,
    cells: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RectUpdate {
    row0: usize,
    row1: usize,
    col0: usize,
    col1: usize,
    seq: Seq,
    cell: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CellUpdate {
    row: usize,
    col: usize,
    seq: Seq,
    cell: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrimUpdate {
    start: usize,
    count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StyleUpdate {
    id: u32,
    seq: Seq,
    fg: u32,
    bg: u32,
    attrs: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireUpdate {
    Row(RowUpdate),
    Rect(RectUpdate),
    Cell(CellUpdate),
    Trim(TrimUpdate),
    Style(StyleUpdate),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireMessage {
    Snapshot {
        lane: Lane,
        watermark: Seq,
        has_more: bool,
        updates: Vec<WireUpdate>,
    },
    Delta {
        watermark: Seq,
        has_more: bool,
        updates: Vec<WireUpdate>,
    },
    SnapshotComplete {
        lane: Lane,
    },
    Done,
}

fn cache_update_to_wire(update: &CacheUpdate) -> WireUpdate {
    match update {
        CacheUpdate::Row(row) => WireUpdate::Row(RowUpdate {
            row: row.row,
            seq: row.seq,
            cells: row.cells.iter().map(|c| (*c).into()).collect(),
        }),
        CacheUpdate::Rect(rect) => WireUpdate::Rect(RectUpdate {
            row0: rect.rows.start,
            row1: rect.rows.end,
            col0: rect.cols.start,
            col1: rect.cols.end,
            seq: rect.seq,
            cell: rect.cell.into_raw(),
        }),
        CacheUpdate::Cell(cell) => WireUpdate::Cell(CellUpdate {
            row: cell.row,
            col: cell.col,
            seq: cell.seq,
            cell: cell.cell.into_raw(),
        }),
        CacheUpdate::Trim(trim) => WireUpdate::Trim(TrimUpdate {
            start: trim.start,
            count: trim.count,
        }),
        CacheUpdate::Style(style) => WireUpdate::Style(StyleUpdate {
            id: style.id.0,
            seq: style.seq,
            fg: style.style.fg,
            bg: style.style.bg,
            attrs: style.style.attrs,
        }),
    }
}

fn wire_update_apply(update: &WireUpdate, grid: &TerminalGrid) {
    match update {
        WireUpdate::Row(row) => {
            for (col, raw) in row.cells.iter().enumerate() {
                let packed = PackedCell::from_raw(*raw);
                let _ = grid.write_packed_cell_if_newer(row.row, col, row.seq, packed);
            }
        }
        WireUpdate::Rect(rect) => {
            let packed = PackedCell::from_raw(rect.cell);
            let _ = grid.fill_rect_with_cell_if_newer(
                rect.row0, rect.col0, rect.row1, rect.col1, rect.seq, packed,
            );
        }
        WireUpdate::Cell(cell) => {
            let packed = PackedCell::from_raw(cell.cell);
            let _ = grid.write_packed_cell_if_newer(cell.row, cell.col, cell.seq, packed);
        }
        WireUpdate::Trim(_) => {
            // trimming is applied via client-side renderer; cache grid ignores
        }
        WireUpdate::Style(style) => {
            let _ = grid.style_table.set(
                StyleId(style.id),
                Style {
                    fg: style.fg,
                    bg: style.bg,
                    attrs: style.attrs,
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
        CacheUpdate::Trim(_) => {
            // grid state ignores trim events in this harness
        }
        CacheUpdate::Style(style) => {
            let _ = grid.style_table.set(style.id, style.style);
        }
    }
}

fn send_wire_message(transport: &dyn Transport, message: &WireMessage) {
    let bytes = bincode::serialize(message).expect("serialize wire message");
    transport.send_bytes(&bytes).expect("transport send");
}

fn recv_wire_message(transport: &dyn Transport) -> WireMessage {
    loop {
        match transport.recv(Duration::from_secs(5)) {
            Ok(TransportMessage {
                payload: Payload::Binary(bytes),
                ..
            }) => {
                return bincode::deserialize(&bytes).expect("deserialize wire message");
            }
            Ok(TransportMessage {
                payload: Payload::Text(_),
                ..
            }) => continue,
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

    let rows = 20;
    let cols = 6;
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

    let delta_stream = Arc::new(TestDeltaStream::new(delta_updates.clone()));

    let config = SyncConfig {
        snapshot_budgets: vec![
            LaneBudget::new(PriorityLane::Foreground, 4),
            LaneBudget::new(PriorityLane::Recent, 6),
            LaneBudget::new(PriorityLane::History, 8),
        ],
        delta_budget: 8,
        heartbeat_interval: Duration::from_millis(50),
    };

    let terminal_sync = Arc::new(TerminalSync::new(
        server_grid.clone(),
        delta_stream,
        config.clone(),
    ));
    let subscription_id = SubscriptionId(42);

    let barrier = Arc::new(Barrier::new(2));

    let server_barrier = barrier.clone();
    let server_grid_clone = server_grid.clone();
    let client_grid_clone = client_grid.clone();
    let delta_updates_clone = delta_updates.clone();

    let server_handle = thread::spawn(move || {
        server_barrier.wait();
        let mut synchronizer = ServerSynchronizer::new(terminal_sync.clone(), config.clone());
        for lane in [
            PriorityLane::Foreground,
            PriorityLane::Recent,
            PriorityLane::History,
        ] {
            while let Some(chunk) = synchronizer.snapshot_chunk(subscription_id, lane) {
                let wire = WireMessage::Snapshot {
                    lane: lane.into(),
                    watermark: chunk.watermark.0,
                    has_more: chunk.has_more,
                    updates: chunk.updates.iter().map(cache_update_to_wire).collect(),
                };
                send_wire_message(server_transport.as_ref(), &wire);
                if !chunk.has_more {
                    break;
                }
            }
            send_wire_message(
                server_transport.as_ref(),
                &WireMessage::SnapshotComplete { lane: lane.into() },
            );
        }

        // Apply delta updates to the server grid (simulate live edits)
        for update in delta_updates_clone.iter() {
            apply_cache_update(update, &server_grid_clone);
        }

        let mut watermark = 0;
        while let Some(batch) = synchronizer.delta_batch(subscription_id, watermark) {
            let wire = WireMessage::Delta {
                watermark: batch.watermark.0,
                has_more: batch.has_more,
                updates: batch.updates.iter().map(cache_update_to_wire).collect(),
            };
            send_wire_message(server_transport.as_ref(), &wire);
            watermark = batch.watermark.0;
            if !batch.has_more {
                break;
            }
        }

        send_wire_message(server_transport.as_ref(), &WireMessage::Done);
    });

    let client_barrier = barrier.clone();
    let client_handle = thread::spawn(move || {
        client_barrier.wait();
        loop {
            let message = recv_wire_message(client_transport.as_ref());
            match message {
                WireMessage::Snapshot { updates, .. } | WireMessage::Delta { updates, .. } => {
                    for update in &updates {
                        wire_update_apply(update, &client_grid_clone);
                    }
                }
                WireMessage::SnapshotComplete { .. } => {}
                WireMessage::Done => break,
            }
        }
    });

    server_handle.join().expect("server thread");
    client_handle.join().expect("client thread");

    assert_grids_match(&server_grid, &client_grid);
}

#[test]
fn terminal_sync_over_all_transports() {
    run_transport_integration(WebRtcBuilder, TransportKind::WebRtc);
    run_transport_integration(WebSocketBuilder, TransportKind::WebSocket);
    run_transport_integration(IpcBuilder, TransportKind::Ipc);
}

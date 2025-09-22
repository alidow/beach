use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use beach_human::cache::GridCache;
use beach_human::cache::terminal::{self, Style, StyleId, TerminalGrid};
use beach_human::model::terminal::diff::{CacheUpdate, RowSnapshot};
use beach_human::sync::terminal::{TerminalDeltaStream, TerminalSync};
use beach_human::sync::{PriorityLane, ServerSynchronizer, SubscriptionId, SyncConfig};
use beach_human::transport::{Transport, TransportKind, TransportMessage, TransportPair};
use serde_json::{Value, json};

#[test]
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
    send_json(
        host_transport.as_ref(),
        json!({
            "type": "hello",
            "subscription": subscription.0,
            "max_seq": hello.max_seq.0,
            "config": encode_sync_config(&hello.config),
        }),
    );
    send_json(
        host_transport.as_ref(),
        json!({"type": "grid", "rows": rows, "cols": cols}),
    );

    for lane in [
        PriorityLane::Foreground,
        PriorityLane::Recent,
        PriorityLane::History,
    ] {
        while let Some(chunk) = synchronizer.snapshot_chunk(subscription, lane) {
            send_json(host_transport.as_ref(), encode_snapshot_chunk(&chunk));
            if !chunk.has_more {
                send_json(
                    host_transport.as_ref(),
                    json!({
                        "type": "snapshot_complete",
                        "subscription": subscription.0,
                        "lane": lane_label(lane),
                    }),
                );
                break;
            }
        }
    }

    let mut client_view: Option<ClientGrid> = None;
    let mut history_complete = false;

    while !history_complete {
        let message = client_transport
            .recv(Duration::from_secs(1))
            .expect("snapshot message");
        let text = message.payload.as_text().expect("text frame");
        let value: Value = serde_json::from_str(text).expect("valid json");
        match value["type"].as_str().unwrap_or("") {
            "hello" => {}
            "grid" => {
                client_view = Some(ClientGrid::new(
                    value["rows"].as_u64().unwrap() as usize,
                    value["cols"].as_u64().unwrap() as usize,
                ));
            }
            "snapshot" => {
                let view = client_view.as_mut().expect("grid message before snapshot");
                for update in value["updates"].as_array().unwrap() {
                    view.apply_update(update);
                }
            }
            "snapshot_complete" => {
                if value["lane"].as_str() == Some("history") {
                    history_complete = true;
                }
            }
            other => panic!("unexpected snapshot message type: {other}"),
        }
    }

    let view = client_view.expect("client view populated");
    assert!(view.contains_row("host% echo hello"));
    assert!(view.contains_row("hello"));
    assert!(view.contains_row("host% "));

    let input_bytes = b"echo world\r";
    let payload = json!({
        "type": "input",
        "seq": 1,
        "data": BASE64.encode(input_bytes),
    })
    .to_string();
    client_transport
        .send(TransportMessage::text(0, payload))
        .expect("send input");

    let inbound = host_transport
        .recv(Duration::from_secs(1))
        .expect("receive input frame");
    let input_text = inbound.payload.as_text().expect("input json");
    let parsed: Value = serde_json::from_str(input_text).expect("input payload json");
    assert_eq!(parsed["type"], "input");
    let decoded = BASE64
        .decode(parsed["data"].as_str().unwrap().as_bytes())
        .expect("decode input payload");
    assert_eq!(decoded.as_slice(), input_bytes);

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

    send_json(
        host_transport.as_ref(),
        json!({"type": "input_ack", "seq": parsed["seq"].clone()}),
    );

    let mut last_seq = hello.max_seq.0;
    while let Some(batch) = synchronizer.delta_batch(subscription, last_seq) {
        if batch.updates.is_empty() {
            break;
        }
        send_json(host_transport.as_ref(), encode_delta_batch(&batch));
        last_seq = batch.watermark.0;
        if !batch.has_more {
            break;
        }
    }

    let mut view = view;
    let mut saw_ack = false;
    let mut saw_world = false;
    for _ in 0..6 {
        let message = client_transport
            .recv(Duration::from_secs(1))
            .expect("delta or ack");
        let text = message.payload.as_text().expect("text frame");
        let value: Value = serde_json::from_str(text).expect("json");
        match value["type"].as_str().unwrap_or("") {
            "input_ack" => saw_ack = true,
            "delta" => {
                for update in value["updates"].as_array().unwrap() {
                    view.apply_update(update);
                }
            }
            other => panic!("unexpected post-input message: {other}"),
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

    fn apply_update(&mut self, update: &Value) {
        match update["kind"].as_str().unwrap_or("") {
            "row" => self.apply_row(update),
            "cell" => self.apply_cell(update),
            "rect" => self.apply_rect(update),
            other => panic!("unknown update kind: {other}"),
        }
    }

    fn apply_row(&mut self, update: &Value) {
        let row = update["row"].as_u64().unwrap() as usize;
        if row >= self.rows {
            return;
        }
        if let Some(text) = update["text"].as_str() {
            let mut chars = text.chars();
            for col in 0..self.cols {
                self.cells[row][col] = chars.next().unwrap_or(' ');
            }
        } else if let Some(cells) = update["cells"].as_array() {
            for col in 0..self.cols {
                let ch = cells
                    .get(col)
                    .and_then(|entry| entry["ch"].as_str())
                    .and_then(|s| s.chars().next())
                    .unwrap_or(' ');
                self.cells[row][col] = ch;
            }
        }
    }

    fn apply_cell(&mut self, update: &Value) {
        let row = update["row"].as_u64().unwrap() as usize;
        let col = update["col"].as_u64().unwrap() as usize;
        if row < self.rows && col < self.cols {
            let ch = update["char"]
                .as_str()
                .unwrap()
                .chars()
                .next()
                .unwrap_or(' ');
            self.cells[row][col] = ch;
        }
    }

    fn apply_rect(&mut self, update: &Value) {
        let rows = update["rows"].as_array().unwrap();
        let cols = update["cols"].as_array().unwrap();
        let row0 = rows[0].as_u64().unwrap() as usize;
        let row1 = rows[1].as_u64().unwrap() as usize;
        let col0 = cols[0].as_u64().unwrap() as usize;
        let col1 = cols[1].as_u64().unwrap() as usize;
        let ch = update["char"]
            .as_str()
            .unwrap()
            .chars()
            .next()
            .unwrap_or(' ');
        for row in row0..row1.min(self.rows) {
            for col in col0..col1.min(self.cols) {
                self.cells[row][col] = ch;
            }
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
                    .eq(needle_chars.into_iter());
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

fn send_json(transport: &dyn Transport, value: Value) {
    let text = value.to_string();
    transport
        .send_text(&text)
        .expect("transport send should succeed");
}

fn encode_snapshot_chunk(chunk: &beach_human::sync::SnapshotChunk<CacheUpdate>) -> Value {
    json!({
        "type": "snapshot",
        "subscription": chunk.subscription_id.0,
        "lane": lane_label(chunk.lane),
        "watermark": chunk.watermark.0,
        "has_more": chunk.has_more,
        "updates": chunk
            .updates
            .iter()
            .map(encode_update)
            .collect::<Vec<_>>(),
    })
}

fn encode_delta_batch(batch: &beach_human::sync::DeltaBatch<CacheUpdate>) -> Value {
    json!({
        "type": "delta",
        "subscription": batch.subscription_id.0,
        "watermark": batch.watermark.0,
        "has_more": batch.has_more,
        "updates": batch
            .updates
            .iter()
            .map(encode_update)
            .collect::<Vec<_>>(),
    })
}

fn encode_sync_config(config: &SyncConfig) -> Value {
    json!({
        "snapshot_budgets": config
            .snapshot_budgets
            .iter()
            .map(|budget| json!({
                "lane": lane_label(budget.lane),
                "max_updates": budget.max_updates,
            }))
            .collect::<Vec<_>>(),
        "delta_budget": config.delta_budget,
        "heartbeat_ms": config.heartbeat_interval.as_millis(),
    })
}

fn lane_label(lane: PriorityLane) -> &'static str {
    match lane {
        PriorityLane::Foreground => "foreground",
        PriorityLane::Recent => "recent",
        PriorityLane::History => "history",
    }
}

fn encode_update(update: &CacheUpdate) -> Value {
    match update {
        CacheUpdate::Cell(cell) => {
            let (ch, style) = terminal::unpack_cell(cell.cell);
            json!({
                "kind": "cell",
                "row": cell.row,
                "col": cell.col,
                "seq": cell.seq,
                "char": ch.to_string(),
                "style": style.0,
            })
        }
        CacheUpdate::Rect(rect) => {
            let (ch, style) = terminal::unpack_cell(rect.cell);
            json!({
                "kind": "rect",
                "rows": [rect.rows.start, rect.rows.end],
                "cols": [rect.cols.start, rect.cols.end],
                "seq": rect.seq,
                "char": ch.to_string(),
                "style": style.0,
            })
        }
        CacheUpdate::Row(row) => {
            json!({
                "kind": "row",
                "row": row.row,
                "seq": row.seq,
                "text": row
                    .cells
                    .iter()
                    .map(|cell| terminal::unpack_cell(*cell).0)
                    .collect::<String>(),
                "cells": row
                    .cells
                    .iter()
                    .map(|cell| {
                        let (ch, style) = terminal::unpack_cell(*cell);
                        json!({
                            "ch": ch.to_string(),
                            "style": style.0,
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        }
        CacheUpdate::Trim(trim) => {
            json!({
                "kind": "trim",
                "start": trim.start,
                "count": trim.count,
            })
        }
        CacheUpdate::Style(style) => {
            json!({
                "kind": "style",
                "id": style.id.0,
                "seq": style.seq,
                "fg": style.style.fg,
                "bg": style.style.bg,
                "attrs": style.style.attrs,
            })
        }
    }
}

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::time::sleep;

use crate::cache::GridCache;
use crate::cache::terminal::{PackedCell, unpack_cell};
use crate::mcp::registry::TerminalSession;
use crate::model::terminal::diff::CacheUpdate;
use crate::sync::{ServerSynchronizer, SubscriptionId};

#[derive(Clone, Debug, Serialize)]
pub struct ResourceDescriptor {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub resource_type: String,
    pub read_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalResource {
    Grid,
    History,
    Cursor,
}

impl TerminalResource {
    pub fn parse(uri: &str) -> Option<Self> {
        if let Some(rest) = uri.strip_prefix("beach://session/") {
            let mut parts = rest.split('/');
            let session = parts.next()?;
            if session.is_empty() {
                return None;
            }
            match parts.collect::<Vec<_>>().as_slice() {
                ["terminal", "grid"] => Some(TerminalResource::Grid),
                ["terminal", "history"] => Some(TerminalResource::History),
                ["terminal", "cursor"] => Some(TerminalResource::Cursor),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn descriptors(session_id: &str) -> Vec<ResourceDescriptor> {
        vec![
            ResourceDescriptor {
                uri: format!("beach://session/{}/terminal/grid", session_id),
                name: "Terminal Grid".to_string(),
                description: Some("Current terminal viewport snapshot".to_string()),
                resource_type: "terminal.grid".to_string(),
                read_only: false,
            },
            ResourceDescriptor {
                uri: format!("beach://session/{}/terminal/history", session_id),
                name: "Terminal History".to_string(),
                description: Some("Scrollback history snapshot".to_string()),
                resource_type: "terminal.history".to_string(),
                read_only: false,
            },
            ResourceDescriptor {
                uri: format!("beach://session/{}/terminal/cursor", session_id),
                name: "Cursor".to_string(),
                description: Some("Latest cursor state".to_string()),
                resource_type: "terminal.cursor".to_string(),
                read_only: true,
            },
        ]
    }
}

#[derive(Debug, Default)]
pub struct GridSnapshotRequest {
    pub top: Option<u64>,
    pub rows: Option<usize>,
}

impl GridSnapshotRequest {
    pub fn from_params(params: Option<&Value>) -> Result<Self> {
        if let Some(value) = params {
            #[derive(Deserialize)]
            struct Helper {
                top: Option<u64>,
                rows: Option<usize>,
            }
            let helper: Helper = serde_json::from_value(value.clone())?;
            Ok(Self {
                top: helper.top,
                rows: helper.rows,
            })
        } else {
            Ok(Self::default())
        }
    }
}

#[derive(Debug)]
pub struct HistoryReadRequest {
    pub start_row: u64,
    pub count: usize,
}

impl HistoryReadRequest {
    pub fn from_params(params: Option<&Value>) -> Result<Self> {
        #[derive(Deserialize)]
        struct Helper {
            start_row: Option<u64>,
            count: Option<usize>,
        }
        let helper: Helper = match params {
            Some(value) => serde_json::from_value(value.clone())?,
            None => Helper {
                start_row: None,
                count: None,
            },
        };
        Ok(Self {
            start_row: helper.start_row.unwrap_or(0),
            count: helper.count.unwrap_or(120).min(1000),
        })
    }
}

pub fn read_grid_snapshot(
    session: &Arc<TerminalSession>,
    request: &GridSnapshotRequest,
) -> Result<Value> {
    let grid = session.sync.grid().clone();
    let (rows, cols) = grid.dims();
    let first_row = grid.first_row_id().unwrap_or(0);
    let last_row = grid.last_row_id().unwrap_or(first_row);

    let desired_rows = request.rows.unwrap_or_else(|| rows.min(80)).max(1);

    let viewport_top = request
        .top
        .or_else(|| last_row.checked_sub(desired_rows as u64 - 1))
        .unwrap_or(first_row);

    let viewport_bottom = viewport_top.saturating_add(desired_rows as u64);

    let mut lines = Vec::new();
    let mut buffer = vec![0u64; cols.max(1)];

    for absolute in viewport_top..viewport_bottom {
        if let Some(index) = grid.index_of_row(absolute) {
            if grid.snapshot_row_into(index, &mut buffer).is_ok() {
                let (text, cells) = render_row(&buffer);
                lines.push(json!({
                    "row": absolute,
                    "text": text,
                    "cells": cells,
                }));
            }
        }
    }

    let cursor = read_cursor_state(session)?;

    Ok(json!({
        "session_id": session.session_id,
        "cols": cols,
        "rows": rows,
        "base_row": first_row,
        "last_row": last_row,
        "viewport": {"top": viewport_top, "rows": desired_rows},
        "lines": lines,
        "cursor": cursor,
    }))
}

pub fn read_history_segment(
    session: &Arc<TerminalSession>,
    request: &HistoryReadRequest,
) -> Result<Value> {
    let grid = session.sync.grid().clone();
    let mut lines = Vec::new();
    let mut buffer = vec![0u64; grid.cols().max(1)];
    for absolute in request.start_row..request.start_row.saturating_add(request.count as u64) {
        if let Some(index) = grid.index_of_row(absolute) {
            if grid.snapshot_row_into(index, &mut buffer).is_ok() {
                let (text, cells) = render_row(&buffer);
                lines.push(json!({
                    "row": absolute,
                    "text": text,
                    "cells": cells,
                }));
            }
        }
    }

    Ok(json!({
        "session_id": session.session_id,
        "start_row": request.start_row,
        "count": request.count,
        "lines": lines,
    }))
}

pub fn read_cursor_state(session: &Arc<TerminalSession>) -> Result<Value> {
    let sync = session.sync.clone();
    let config = sync.config().clone();
    let synchronizer = ServerSynchronizer::new(sync.clone(), config);
    let hello = synchronizer.hello(SubscriptionId(0));
    let since = hello.max_seq.0.saturating_sub(1024);
    if let Some(batch) = synchronizer.delta_batch(SubscriptionId(0), since) {
        for update in batch.updates.iter().rev() {
            if let CacheUpdate::Cursor(cursor) = update {
                return Ok(json!({
                    "row": cursor.row,
                    "col": cursor.col,
                    "seq": cursor.seq,
                    "visible": cursor.visible,
                    "blink": cursor.blink,
                }));
            }
        }
    }
    Ok(json!({
        "row": Value::Null,
        "col": Value::Null,
        "visible": false,
        "blink": false
    }))
}

pub fn spawn_grid_subscription(
    session: Arc<TerminalSession>,
    request: GridSnapshotRequest,
    tx: Sender<Value>,
    subscription_id: String,
    mut cancel_rx: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sync = session.sync.clone();
        let synchronizer = ServerSynchronizer::new(sync.clone(), sync.config().clone());
        let subscription = SubscriptionId(1);
        let mut last_seq = 0u64;

        if let Ok(snapshot) = read_grid_snapshot(&session, &request) {
            let _ = tx
                .send(json!({
                    "jsonrpc": "2.0",
                    "method": "resources/updated",
                    "params": {
                        "subscription_id": subscription_id,
                        "event": "snapshot",
                        "resource": {
                            "uri": format!("beach://session/{}/terminal/grid", session.session_id)
                        },
                        "data": snapshot,
                    }
                }))
                .await;
        }

        loop {
            let mut dispatched = false;
            if let Some(batch) = synchronizer.delta_batch(subscription, last_seq) {
                if !batch.updates.is_empty() {
                    let payload = updates_to_json(&batch.updates);
                    let _ = tx
                        .send(json!({
                            "jsonrpc": "2.0",
                            "method": "resources/updated",
                            "params": {
                                "subscription_id": subscription_id,
                                "event": "delta",
                                "resource": {
                                    "uri": format!("beach://session/{}/terminal/grid", session.session_id)
                                },
                                "data": {
                                    "watermark": batch.watermark.0,
                                    "updates": payload,
                                }
                            }
                        }))
                        .await;
                    dispatched = true;
                    last_seq = batch.watermark.0;
                }
            }

            tokio::select! {
                _ = &mut cancel_rx => {
                    break;
                }
                _ = sleep(Duration::from_millis(if dispatched { 15 } else { 60 })) => {}
            }
        }
    })
}

fn render_row(buffer: &[u64]) -> (String, Vec<Value>) {
    let mut text = String::with_capacity(buffer.len());
    let mut cells = Vec::with_capacity(buffer.len());
    for cell in buffer {
        let packed = PackedCell::from(*cell);
        let (ch, style) = unpack_cell(packed);
        text.push(ch);
        cells.push(json!({"ch": ch, "style": style.0}));
    }
    while text.ends_with(' ') {
        text.pop();
    }
    (text, cells)
}

fn updates_to_json(updates: &[CacheUpdate]) -> Vec<Value> {
    updates
        .iter()
        .filter_map(|update| match update {
            CacheUpdate::Cell(cell) => Some(json!({
                "type": "cell",
                "row": cell.row,
                "col": cell.col,
                "seq": cell.seq,
                "cell": u64::from(cell.cell),
            })),
            CacheUpdate::Rect(rect) => Some(json!({
                "type": "rect",
                "rows": {"start": rect.rows.start, "end": rect.rows.end},
                "cols": {"start": rect.cols.start, "end": rect.cols.end},
                "seq": rect.seq,
                "cell": u64::from(rect.cell),
            })),
            CacheUpdate::Row(row) => {
                let cells: Vec<u64> = row.cells.iter().map(|cell| (*cell).into()).collect();
                Some(json!({
                    "type": "row",
                    "row": row.row,
                    "seq": row.seq,
                    "cells": cells,
                }))
            }
            CacheUpdate::Trim(trim) => Some(json!({
                "type": "trim",
                "start": trim.start,
                "count": trim.count,
            })),
            CacheUpdate::Style(style) => Some(json!({
                "type": "style",
                "id": style.id.0,
                "seq": style.seq,
                "fg": style.style.fg,
                "bg": style.style.bg,
                "attrs": style.style.attrs,
            })),
            CacheUpdate::Cursor(cursor) => Some(json!({
                "type": "cursor",
                "row": cursor.row,
                "col": cursor.col,
                "seq": cursor.seq,
                "visible": cursor.visible,
                "blink": cursor.blink,
            })),
        })
        .collect()
}

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;

pub mod wire;

pub use wire::{
    WireError, binary_protocol_enabled, decode_client_frame_binary, decode_host_frame_binary,
    encode_client_frame_binary, encode_host_frame_binary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Lane {
    Foreground = 0,
    Recent = 1,
    History = 2,
}

impl Lane {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneBudgetFrame {
    pub lane: Lane,
    pub max_updates: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConfigFrame {
    pub snapshot_budgets: Vec<LaneBudgetFrame>,
    pub delta_budget: u32,
    pub heartbeat_ms: u64,
    pub initial_snapshot_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Update {
    Cell {
        row: u32,
        col: u32,
        seq: u64,
        cell: u64,
    },
    Rect {
        rows: [u32; 2],
        cols: [u32; 2],
        seq: u64,
        cell: u64,
    },
    Row {
        row: u32,
        seq: u64,
        cells: Vec<u64>,
    },
    RowSegment {
        row: u32,
        start_col: u32,
        seq: u64,
        cells: Vec<u64>,
    },
    Trim {
        start: u32,
        count: u32,
        seq: u64,
    },
    Style {
        id: u32,
        seq: u64,
        fg: u32,
        bg: u32,
        attrs: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFrame {
    Heartbeat {
        seq: u64,
        timestamp_ms: u64,
    },
    Hello {
        subscription: u64,
        max_seq: u64,
        config: SyncConfigFrame,
    },
    Grid {
        viewport_rows: u32,
        cols: u32,
        history_rows: u32,
    },
    Snapshot {
        subscription: u64,
        lane: Lane,
        watermark: u64,
        has_more: bool,
        updates: Vec<Update>,
    },
    SnapshotComplete {
        subscription: u64,
        lane: Lane,
    },
    Delta {
        subscription: u64,
        watermark: u64,
        has_more: bool,
        updates: Vec<Update>,
    },
    HistoryBackfill {
        subscription: u64,
        request_id: u64,
        start_row: u64,
        count: u32,
        updates: Vec<Update>,
        more: bool,
    },
    InputAck {
        seq: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Input {
        seq: u64,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    RequestBackfill {
        subscription: u64,
        request_id: u64,
        start_row: u64,
        count: u32,
    },
    #[serde(other)]
    Unknown,
}

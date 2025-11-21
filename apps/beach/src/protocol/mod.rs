use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 2;
pub const FEATURE_CURSOR_SYNC: u32 = 1 << 0;

pub mod terminal;
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
pub struct CursorFrame {
    pub row: u32,
    pub col: u32,
    pub seq: u64,
    pub visible: bool,
    pub blink: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionFrame {
    pub namespace: String,
    pub kind: String,
    pub payload: Bytes,
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
        features: u32,
    },
    Grid {
        cols: u32,
        history_rows: u32,
        base_row: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        viewport_rows: Option<u32>,
    },
    Snapshot {
        subscription: u64,
        lane: Lane,
        watermark: u64,
        has_more: bool,
        updates: Vec<Update>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<CursorFrame>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<CursorFrame>,
    },
    HistoryBackfill {
        subscription: u64,
        request_id: u64,
        start_row: u64,
        count: u32,
        updates: Vec<Update>,
        more: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<CursorFrame>,
    },
    InputAck {
        seq: u64,
    },
    Cursor {
        subscription: u64,
        cursor: CursorFrame,
    },
    Extension {
        #[serde(flatten)]
        frame: ExtensionFrame,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ViewportCommand {
    Clear = 0,
}

impl ViewportCommand {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ViewportCommand::Clear),
            _ => None,
        }
    }
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
    ViewportCommand {
        command: ViewportCommand,
    },
    Extension {
        #[serde(flatten)]
        frame: ExtensionFrame,
    },
    #[serde(other)]
    Unknown,
}

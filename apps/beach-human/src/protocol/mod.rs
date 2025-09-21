use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u8 = 1;

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
        rows: u32,
        cols: u32,
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
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

pub fn encode_host_frame(frame: &HostFrame) -> Result<Vec<u8>, FrameError> {
    rmp_serde::to_vec_named(frame).map_err(FrameError::from)
}

pub fn decode_host_frame(bytes: &[u8]) -> Result<HostFrame, FrameError> {
    rmp_serde::from_slice(bytes).map_err(FrameError::from)
}

pub fn encode_client_frame(frame: &ClientFrame) -> Result<Vec<u8>, FrameError> {
    rmp_serde::to_vec_named(frame).map_err(FrameError::from)
}

pub fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame, FrameError> {
    rmp_serde::from_slice(bytes).map_err(FrameError::from)
}

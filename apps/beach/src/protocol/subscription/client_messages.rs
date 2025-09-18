use super::messages::{CompressionType, Dimensions, Prefetch, ViewMode, ViewPosition, Viewport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe {
        subscription_id: String,
        dimensions: Dimensions,

        // New unified grid fields
        #[serde(skip_serializing_if = "Option::is_none")]
        initial_fetch_size: Option<u32>, // NEW: How many rows to send initially (e.g., 500)
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_history: Option<bool>, // NEW: Whether to stream full history

        // Legacy viewport-based subscription fields (still supported)
        #[serde(skip_serializing_if = "Option::is_none")]
        viewport: Option<Viewport>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefetch: Option<Prefetch>,
        #[serde(skip_serializing_if = "Option::is_none")]
        follow_tail: Option<bool>,

        // DEPRECATED: kept for backward compatibility
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<ViewMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<ViewPosition>,

        #[serde(skip_serializing_if = "Option::is_none")]
        compression: Option<CompressionType>,
    },
    /// Fast viewport update for scrolling
    ViewportChanged {
        subscription_id: String,
        viewport: Viewport,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefetch: Option<Prefetch>,
        #[serde(skip_serializing_if = "Option::is_none")]
        follow_tail: Option<bool>,
    },
    /// DEPRECATED: Use ViewportChanged instead
    ModifySubscription {
        subscription_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        dimensions: Option<Dimensions>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<ViewMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<ViewPosition>,
    },
    Unsubscribe {
        subscription_id: String,
    },
    TerminalInput {
        data: Vec<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        echo_local: Option<bool>,
    },
    RequestState {
        subscription_id: String,
        #[serde(rename = "request_type")]
        request_type: StateRequestType,
        #[serde(skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
    },
    Acknowledge {
        message_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum: Option<u32>,
    },
    Control {
        #[serde(rename = "control_type")]
        control_type: ControlType,
        #[serde(skip_serializing_if = "Option::is_none")]
        subscription_id: Option<String>,
    },
    Ping {
        timestamp: i64,
        subscriptions: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StateRequestType {
    Snapshot,
    Checkpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlType {
    Pause,
    Resume,
    ClearHistory,
}

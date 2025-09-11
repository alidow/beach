use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use super::messages::{
    ViewMode, SubscriptionStatus, ErrorCode, NotificationType
};
use crate::server::terminal_state::{Grid, GridDelta};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot {
        subscription_id: String,
        sequence: u64,
        grid: Grid,
        timestamp: i64,
        checksum: u32,
    },
    Delta {
        subscription_id: String,
        sequence: u64,
        changes: GridDelta,
        timestamp: i64,
    },
    DeltaBatch {
        subscription_id: String,
        start_sequence: u64,
        end_sequence: u64,
        deltas: Vec<GridDelta>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compressed: Option<bool>,
    },
    ViewTransition {
        subscription_id: String,
        from_mode: ViewMode,
        to_mode: ViewMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        delta: Option<GridDelta>,
        #[serde(skip_serializing_if = "Option::is_none")]
        snapshot: Option<Grid>,
    },
    SubscriptionAck {
        subscription_id: String,
        status: SubscriptionStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        shared_with: Option<Vec<String>>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        subscription_id: Option<String>,
        code: ErrorCode,
        message: String,
        recoverable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after: Option<u32>,
    },
    Pong {
        timestamp: i64,
        server_sequence: u64,
        subscriptions: HashMap<String, SubscriptionInfo>,
    },
    Notify {
        #[serde(rename = "notification_type")]
        notification_type: NotificationType,
        #[serde(skip_serializing_if = "Option::is_none")]
        subscription_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    HistoryInfo {
        subscription_id: String,
        /// Oldest available line number in history
        oldest_line: u64,
        /// Most recent line number
        latest_line: u64,
        /// Total number of lines in history
        total_lines: u64,
        /// Oldest available timestamp
        #[serde(skip_serializing_if = "Option::is_none")]
        oldest_timestamp: Option<i64>,
        /// Most recent timestamp
        #[serde(skip_serializing_if = "Option::is_none")]
        latest_timestamp: Option<i64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionInfo {
    pub sequence: u64,
    pub mode: ViewMode,
}
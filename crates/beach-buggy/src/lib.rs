//! Beach Buggy: high-performance harness runtime shared by Beach clients.
//!
//! Responsibilities:
//! - registering harness capabilities with Beach Manager
//! - streaming state diffs (terminal, GUI metadata, structured OCR)
//! - receiving and acknowledging action commands with tight latency budgets
//! - providing reusable codecs for diffs and compression

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessRegistration {
    pub session_id: String,
    pub harness_type: HarnessType,
    pub capabilities: Vec<String>,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessType {
    TerminalShim,
    CabanaAdapter,
    RemoteWidget,
    ServiceProxy,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateDiff {
    pub sequence: u64,
    pub emitted_at: SystemTime,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCommand {
    pub id: String,
    pub action_type: String,
    pub payload: serde_json::Value,
    pub expires_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionAck {
    pub id: String,
    pub status: AckStatus,
    pub applied_at: SystemTime,
    pub latency_ms: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckStatus {
    Ok,
    Rejected,
    Expired,
    Preempted,
}

/// Portable control messages for dual-channel architecture
/// These messages are designed to be identical across Rust and TypeScript clients
use serde::{Deserialize, Serialize};

/// Messages that MUST go through reliable control channel
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Input from client with sequence number for predictive echo
    Input {
        client_id: String,
        client_seq: u64,
        bytes: Vec<u8>,
    },

    /// Server acknowledgment of input with ordering info
    InputAck {
        client_seq: u64,
        apply_seq: u64,
        version: u64,
    },

    /// Client acknowledgment of last applied version
    Ack { version: u64 },

    /// Request for resynchronization
    ResyncRequest { reason: String },

    /// Client viewport dimensions
    Viewport { cols: u16, rows: u16 },

    /// Subscription window for overscan
    Subscribe { from_line: u64, height: u16 },

    /// Heartbeat for connection monitoring
    Heartbeat { t: i64 },

    /// Heartbeat acknowledgment
    HeartbeatAck { t: i64 },
}

/// Messages that CAN go through unreliable output channel
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputMessage {
    /// Delta update with versioning
    Delta {
        base_version: u64,
        next_version: u64,
        delta: serde_json::Value, // GridDelta serialized
    },

    /// Full snapshot for resync
    Snapshot {
        version: u64,
        grid: serde_json::Value, // Grid serialized
        compressed: bool,
    },

    /// Hash for integrity checking
    Hash { version: u64, h: Vec<u8> },
}

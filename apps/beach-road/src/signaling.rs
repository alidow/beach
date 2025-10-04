use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Supported P2P transport types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    #[serde(rename = "webrtc")]
    WebRTC,
    WebTransport,
    Direct,         // Direct TCP/UDP connection
    Custom(String), // For future extensions
}

/// Transport-specific signaling data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportSignal {
    /// WebRTC-specific signals
    #[serde(rename = "webrtc")]
    WebRTC {
        #[serde(flatten)]
        signal: WebRTCSignal,
    },
    /// WebTransport-specific signals
    #[serde(rename = "webtransport")]
    WebTransport {
        #[serde(flatten)]
        signal: WebTransportSignal,
    },
    /// Direct connection signals
    #[serde(rename = "direct")]
    Direct {
        #[serde(flatten)]
        signal: DirectSignal,
    },
    /// Generic signal for custom transports
    #[serde(rename = "custom")]
    Custom {
        transport_name: String,
        signal_type: String,
        payload: serde_json::Value,
    },
}

/// WebRTC-specific signaling messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal_type", rename_all = "snake_case")]
pub enum WebRTCSignal {
    Offer {
        sdp: String,
        handshake_id: String,
    },
    Answer {
        sdp: String,
        handshake_id: String,
    },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u32>,
        handshake_id: String,
    },
}

/// WebTransport-specific signaling messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal_type", rename_all = "snake_case")]
pub enum WebTransportSignal {
    ServerUrl { url: String },
    Certificate { fingerprint: String },
}

/// Direct connection signaling messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal_type", rename_all = "snake_case")]
pub enum DirectSignal {
    Address { host: String, port: u16 },
    PublicKey { key: String },
}

/// Messages sent from client to session server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Join a session with optional passphrase and transport capabilities
    Join {
        peer_id: String,
        passphrase: Option<String>,
        /// Transports this peer supports
        supported_transports: Vec<TransportType>,
        /// Preferred transport order
        preferred_transport: Option<TransportType>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        mcp: bool,
    },
    /// Negotiate transport to use with a peer
    NegotiateTransport {
        to_peer: String,
        proposed_transport: TransportType,
    },
    /// Accept transport negotiation
    AcceptTransport {
        to_peer: String,
        transport: TransportType,
    },
    /// Send transport-specific signaling data
    Signal {
        to_peer: String,
        signal: serde_json::Value,
    },
    /// Heartbeat to keep connection alive
    Ping,
    /// Debug request for terminal state
    Debug { request: DebugRequest },
}

/// Messages sent from session server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Acknowledge successful join
    JoinSuccess {
        session_id: String,
        peer_id: String,
        peers: Vec<PeerInfo>,
        /// Common transports supported by all peers
        available_transports: Vec<TransportType>,
    },
    /// Join failed
    JoinError { reason: String },
    /// New peer joined the session
    PeerJoined { peer: PeerInfo },
    /// Peer left the session
    PeerLeft { peer_id: String },
    /// Transport negotiation request from another peer
    TransportProposal {
        from_peer: String,
        proposed_transport: TransportType,
    },
    /// Transport negotiation accepted
    TransportAccepted {
        from_peer: String,
        transport: TransportType,
    },
    /// Received transport-specific signal from another peer
    Signal {
        from_peer: String,
        signal: serde_json::Value,
    },
    /// Response to ping
    Pong,
    /// Error message
    Error { message: String },
    /// Debug response with terminal state
    Debug { response: DebugResponse },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcSdpPayload {
    pub sdp: String,
    #[serde(rename = "type")]
    pub typ: String,
    pub handshake_id: String,
    pub from_peer: String,
    pub to_peer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub role: PeerRole,
    pub joined_at: i64,
    /// Transports this peer supports
    pub supported_transports: Vec<TransportType>,
    /// Peer's preferred transport
    pub preferred_transport: Option<TransportType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    Server,
    Client,
}

/// Generate a unique peer ID
pub fn generate_peer_id() -> String {
    Uuid::new_v4().to_string()
}

/// Debug request types for terminal state inspection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebugRequest {
    /// Request current grid view
    GetGridView {
        /// Optional number of rows to return
        height: Option<u16>,
        /// Optional time to get historical view
        #[serde(skip_serializing_if = "Option::is_none")]
        at_time: Option<DateTime<Utc>>,
        /// Optional line number to start from
        #[serde(skip_serializing_if = "Option::is_none")]
        from_line: Option<u64>,
    },
    /// Get terminal state statistics
    GetStats,
    /// Clear terminal history
    ClearHistory,
}

/// Debug response types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebugResponse {
    /// Grid view response
    GridView {
        /// The grid dimensions
        width: u16,
        height: u16,
        /// Cursor position
        cursor_row: u16,
        cursor_col: u16,
        cursor_visible: bool,
        /// The grid content as rows of text
        rows: Vec<String>,
        /// Optional ANSI-colored version for terminal rendering
        ansi_rows: Option<Vec<String>>,
        /// Metadata
        timestamp: DateTime<Utc>,
        start_line: u64,
        end_line: u64,
    },
    /// Terminal statistics
    Stats {
        history_size_bytes: usize,
        total_deltas: u64,
        total_snapshots: usize,
        current_dimensions: (u16, u16),
        session_duration_secs: u64,
    },
    /// Success response
    Success { message: String },
    /// Error response
    Error { message: String },
}

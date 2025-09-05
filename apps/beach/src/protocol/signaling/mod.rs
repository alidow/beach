use serde::{Deserialize, Serialize};

/// Re-export transport types from session server protocol
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    WebRTC,
    WebTransport,
    Direct,
    Custom(String),
}

/// Messages sent from beach to session server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        peer_id: String,
        passphrase: Option<String>,
        supported_transports: Vec<TransportType>,
        preferred_transport: Option<TransportType>,
    },
    Signal {
        to_peer: String,
        signal: serde_json::Value, // Will be TransportSignal once implemented
    },
    Ping,
    /// Debug request for terminal state
    Debug {
        request: serde_json::Value, // Will contain DebugRequest
    },
}

/// Messages received from session server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    JoinSuccess {
        session_id: String,
        peer_id: String,
        peers: Vec<PeerInfo>,
        available_transports: Vec<TransportType>,
    },
    JoinError {
        reason: String,
    },
    PeerJoined {
        peer: PeerInfo,
    },
    PeerLeft {
        peer_id: String,
    },
    Signal {
        from_peer: String,
        signal: serde_json::Value,
    },
    Pong,
    Error {
        message: String,
    },
    /// Debug response with terminal state
    Debug {
        response: serde_json::Value, // Will contain DebugResponse
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: String,
    pub role: PeerRole,
    pub joined_at: i64,
    pub supported_transports: Vec<TransportType>,
    pub preferred_transport: Option<TransportType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    Server,
    Client,
}

/// Application-level messages between beach server and clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    /// Terminal output from server to clients
    TerminalOutput {
        data: Vec<u8>,
    },
    /// Terminal input from client to server
    TerminalInput {
        data: Vec<u8>,
    },
    /// Terminal resize event
    TerminalResize {
        cols: u16,
        rows: u16,
    },
    /// Protocol message for subscription system
    Protocol {
        #[serde(flatten)]
        message: serde_json::Value, // Will contain ClientMessage or ServerMessage
    },
    /// Custom application message
    Custom {
        payload: serde_json::Value,
    },
    /// Debug message for terminal state
    Debug {
        request: serde_json::Value, // DebugRequest
        response: Option<serde_json::Value>, // Optional DebugResponse
    },
}
pub mod auth;
pub mod cache;
pub mod client;
pub mod debug;
pub mod mcp;
pub mod model;
pub mod protocol;
pub mod server;
pub mod session;
pub mod sync;
pub mod telemetry;
pub mod terminal;
pub mod transport;

pub use crate::cache::{
    CellSnapshot, GridCache, Seq, WriteError, WriteOutcome,
    terminal::{
        TerminalGrid,
        packed::{PackedCell, Style, StyleId, StyleTable, pack_cell, unpack_cell},
    },
};
pub use crate::protocol::{
    ClientFrame, HostFrame, decode_host_frame_binary, encode_client_frame_binary,
};
pub use crate::session::{
    HostSession, JoinedSession, SessionConfig, SessionError, SessionHandle, SessionManager,
    SessionRole, TransportOffer,
};
pub use crate::terminal::error::CliError;
pub use crate::transport::terminal::negotiation::{
    NegotiatedSingle, NegotiatedTransport, negotiate_transport,
};
pub use crate::transport::webrtc::{SignalingClient, WebRtcChannels, WebRtcConnection, WebRtcRole};
pub use crate::transport::{
    Payload, Transport, TransportError, TransportId, TransportKind, TransportMessage,
};

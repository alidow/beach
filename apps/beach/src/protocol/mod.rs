pub mod control_messages;
pub mod signaling;
pub mod subscription;

// Re-export subscription protocol types for backward compatibility
pub use subscription::{
    messages::{ViewMode, ViewPosition, Dimensions, CompressionType, ErrorCode, SubscriptionStatus, NotificationType},
    client_messages::{ClientMessage, StateRequestType, ControlType},
    server_messages::{ServerMessage, SubscriptionInfo},
};

// Re-export control messages for dual-channel architecture
pub use control_messages::{ControlMessage, OutputMessage};
pub mod control_messages;
pub mod signaling;
pub mod subscription;

// Re-export subscription protocol types for backward compatibility
pub use subscription::{
    client_messages::{ClientMessage, ControlType, StateRequestType},
    messages::{
        CompressionType, Dimensions, ErrorCode, NotificationType, SubscriptionStatus, ViewMode,
        ViewPosition,
    },
    server_messages::{ServerMessage, SubscriptionInfo},
};

// Re-export control messages for dual-channel architecture
pub use control_messages::{ControlMessage, OutputMessage};

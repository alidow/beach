use anyhow::Result;
use async_trait::async_trait;

/// Reliability configuration for a transport channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelReliability {
    /// Reliable, ordered delivery (TCP-like)
    Reliable,
    /// Unreliable, unordered delivery (UDP-like)
    Unreliable {
        /// Maximum number of retransmission attempts
        max_retransmits: Option<u16>,
        /// Maximum packet lifetime in milliseconds
        max_packet_lifetime: Option<u16>,
    },
}

/// Purpose of a channel, used for routing messages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelPurpose {
    /// Control channel for handshake, auth, acks, resync
    Control,
    /// Input channel for raw terminal input (fast, no serialization)
    Input,
    /// Output channel for terminal frames (lossy allowed)
    Output,
    /// Custom channel for future extensions
    Custom(u8),
}

impl ChannelPurpose {
    /// Get the standard label for this channel purpose
    pub fn label(&self) -> String {
        match self {
            ChannelPurpose::Control => "beach/ctrl/1".to_string(),
            ChannelPurpose::Input => "beach/input/1".to_string(),
            ChannelPurpose::Output => "beach/term/1".to_string(),
            ChannelPurpose::Custom(n) => format!("beach/custom/{}", n),
        }
    }

    /// Get the recommended reliability for this channel purpose
    pub fn default_reliability(&self) -> ChannelReliability {
        match self {
            ChannelPurpose::Control => ChannelReliability::Reliable,
            ChannelPurpose::Input => ChannelReliability::Reliable, // Input must be reliable & ordered
            ChannelPurpose::Output => ChannelReliability::Unreliable {
                max_retransmits: Some(0),
                max_packet_lifetime: None,
            },
            ChannelPurpose::Custom(_) => ChannelReliability::Reliable,
        }
    }
}

/// Individual channel within a transport
#[async_trait]
pub trait TransportChannel: Send + Sync {
    /// Get the channel label
    fn label(&self) -> &str;

    /// Get the channel reliability configuration
    fn reliability(&self) -> ChannelReliability;

    /// Get the channel purpose
    fn purpose(&self) -> ChannelPurpose;

    /// Send data through this channel
    async fn send(&self, data: &[u8]) -> Result<()>;

    /// Receive data from this channel
    async fn recv(&mut self) -> Option<Vec<u8>>;

    /// Check if the channel is open
    fn is_open(&self) -> bool;

    /// Get channel statistics (optional)
    fn stats(&self) -> ChannelStats {
        ChannelStats::default()
    }
}

/// Statistics for a transport channel
#[derive(Debug, Clone, Default)]
pub struct ChannelStats {
    /// Number of bytes sent
    pub bytes_sent: u64,
    /// Number of bytes received
    pub bytes_received: u64,
    /// Number of messages sent
    pub messages_sent: u64,
    /// Number of messages received
    pub messages_received: u64,
    /// Number of send errors
    pub send_errors: u64,
    /// Number of receive errors
    pub recv_errors: u64,
}

/// Channel creation options
#[derive(Debug, Clone)]
pub struct ChannelOptions {
    /// Channel purpose
    pub purpose: ChannelPurpose,
    /// Channel reliability
    pub reliability: Option<ChannelReliability>,
    /// Custom label (overrides default)
    pub label: Option<String>,
}

impl ChannelOptions {
    /// Create options for a control channel
    pub fn control() -> Self {
        Self {
            purpose: ChannelPurpose::Control,
            reliability: None,
            label: None,
        }
    }

    /// Create options for an input channel
    pub fn input() -> Self {
        Self {
            purpose: ChannelPurpose::Input,
            reliability: None,
            label: None,
        }
    }

    /// Create options for an output channel
    pub fn output() -> Self {
        Self {
            purpose: ChannelPurpose::Output,
            reliability: None,
            label: None,
        }
    }

    /// Set custom reliability
    pub fn with_reliability(mut self, reliability: ChannelReliability) -> Self {
        self.reliability = Some(reliability);
        self
    }

    /// Set custom label
    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }
}

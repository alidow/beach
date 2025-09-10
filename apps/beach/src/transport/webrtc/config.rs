use crate::transport::TransportMode;
use crate::debug_log::DebugLogger;
use webrtc::ice_transport::ice_server::RTCIceServer;

/// Configuration for WebRTC transport
#[derive(Clone)]
pub struct WebRTCConfig {
    /// Transport mode (Server or Client)
    pub mode: TransportMode,
    /// ICE servers for connection establishment
    pub ice_servers: Vec<RTCIceServer>,
    /// Data channel label
    pub data_channel_label: String,
    /// Whether the data channel should be ordered
    pub ordered: bool,
    /// Maximum number of retransmissions for unreliable channels
    pub max_retransmits: Option<u16>,
    /// Optional debug logger
    pub debug_logger: Option<DebugLogger>,
}

impl Default for WebRTCConfig {
    fn default() -> Self {
        Self {
            mode: TransportMode::Client,
            ice_servers: vec![
                // Default STUN server for NAT traversal
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                },
            ],
            data_channel_label: "beach-data".to_string(),
            ordered: true,
            max_retransmits: None, // Reliable channel by default
            debug_logger: None,
        }
    }
}

impl WebRTCConfig {
    /// Create a new WebRTC configuration
    pub fn new(mode: TransportMode) -> Self {
        // Check if localhost-only mode is requested
        let ice_servers = if std::env::var("BEACH_LOCALHOST_ONLY").is_ok() {
            vec![] // No ICE servers for localhost
        } else {
            vec![
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                },
            ]
        };
        
        Self {
            mode,
            ice_servers,
            ..Default::default()
        }
    }
    
    /// Create a localhost-only configuration (no STUN/TURN)
    pub fn localhost(mode: TransportMode) -> Self {
        Self {
            mode,
            ice_servers: vec![], // No ICE servers for localhost
            ..Default::default()
        }
    }
    
    /// Set the debug logger
    pub fn with_debug_logger(mut self, logger: DebugLogger) -> Self {
        self.debug_logger = Some(logger);
        self
    }
}

/// Builder for WebRTC configuration
pub struct WebRTCConfigBuilder {
    mode: Option<TransportMode>,
    ice_servers: Vec<RTCIceServer>,
    data_channel_label: Option<String>,
    ordered: Option<bool>,
    max_retransmits: Option<u16>,
    debug_logger: Option<DebugLogger>,
}

impl WebRTCConfigBuilder {
    pub fn new() -> Self {
        Self {
            mode: None,
            ice_servers: vec![],
            data_channel_label: None,
            ordered: None,
            max_retransmits: None,
            debug_logger: None,
        }
    }
    
    pub fn mode(mut self, mode: TransportMode) -> Self {
        self.mode = Some(mode);
        self
    }
    
    pub fn add_ice_server(mut self, urls: Vec<String>) -> Self {
        self.ice_servers.push(RTCIceServer {
            urls,
            ..Default::default()
        });
        self
    }
    
    pub fn add_ice_server_with_credentials(
        mut self, 
        urls: Vec<String>, 
        username: String, 
        credential: String
    ) -> Self {
        self.ice_servers.push(RTCIceServer {
            urls,
            username,
            credential,
            ..Default::default()
        });
        self
    }
    
    pub fn data_channel_label(mut self, label: String) -> Self {
        self.data_channel_label = Some(label);
        self
    }
    
    pub fn ordered(mut self, ordered: bool) -> Self {
        self.ordered = Some(ordered);
        self
    }
    
    pub fn max_retransmits(mut self, max_retransmits: u16) -> Self {
        self.max_retransmits = Some(max_retransmits);
        self
    }
    
    pub fn debug_logger(mut self, logger: Option<DebugLogger>) -> Self {
        self.debug_logger = logger;
        self
    }
    
    pub fn build(self) -> Result<WebRTCConfig, String> {
        let mode = self.mode.ok_or("Transport mode is required")?;
        let mut config = WebRTCConfig::new(mode);
        
        if !self.ice_servers.is_empty() {
            config.ice_servers = self.ice_servers;
        }
        
        if let Some(label) = self.data_channel_label {
            config.data_channel_label = label;
        }
        
        if let Some(ordered) = self.ordered {
            config.ordered = ordered;
        }
        
        if let Some(max_retransmits) = self.max_retransmits {
            config.max_retransmits = Some(max_retransmits);
        }
        
        config.debug_logger = self.debug_logger;
        
        Ok(config)
    }
}
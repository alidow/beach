use crate::transport::TransportMode;

/// Configuration for WebSocket transport
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// The WebSocket URL or base URL
    pub url: String,
    /// Optional path suffix (e.g., "/ws/session123")
    pub path: Option<String>,
    /// Transport mode (Server or Client)
    pub mode: TransportMode,
    /// Whether to use TLS (wss:// vs ws://)
    pub use_tls: bool,
}

impl WebSocketConfig {
    /// Create a new WebSocket configuration
    pub fn new(url: String, mode: TransportMode) -> Self {
        // Auto-detect TLS based on URL
        let use_tls = url.starts_with("wss://")
            || (!url.starts_with("ws://")
                && !url.contains("127.0.0.1")
                && !url.contains("localhost"));

        Self {
            url,
            path: None,
            mode,
            use_tls,
        }
    }

    /// Set the path suffix
    pub fn with_path(mut self, path: String) -> Self {
        self.path = Some(path);
        self
    }

    /// Build the full WebSocket URL
    pub fn build_url(&self) -> String {
        let mut url = self.url.clone();

        // Normalize URL
        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            url = if self.use_tls {
                format!("wss://{}", url)
            } else {
                format!("ws://{}", url)
            };
        }

        // Normalize localhost to avoid IPv6 issues
        if url.contains("localhost") {
            url = url.replace("localhost", "127.0.0.1");
        }

        // Add path if provided
        if let Some(ref path) = self.path {
            if !url.ends_with('/') && !path.starts_with('/') {
                url.push('/');
            }
            url.push_str(path);
        }

        url
    }
}

/// Builder for WebSocket configuration
pub struct WebSocketConfigBuilder {
    url: Option<String>,
    path: Option<String>,
    mode: Option<TransportMode>,
    use_tls: Option<bool>,
}

impl WebSocketConfigBuilder {
    pub fn new() -> Self {
        Self {
            url: None,
            path: None,
            mode: None,
            use_tls: None,
        }
    }

    pub fn url(mut self, url: String) -> Self {
        self.url = Some(url);
        self
    }

    pub fn path(mut self, path: String) -> Self {
        self.path = Some(path);
        self
    }

    pub fn mode(mut self, mode: TransportMode) -> Self {
        self.mode = Some(mode);
        self
    }

    pub fn use_tls(mut self, use_tls: bool) -> Self {
        self.use_tls = Some(use_tls);
        self
    }

    pub fn build(self) -> Result<WebSocketConfig, String> {
        let url = self.url.ok_or("URL is required")?;
        let mode = self.mode.unwrap_or(TransportMode::Client);

        let mut config = WebSocketConfig::new(url, mode);

        if let Some(path) = self.path {
            config = config.with_path(path);
        }

        if let Some(use_tls) = self.use_tls {
            config.use_tls = use_tls;
        }

        Ok(config)
    }
}

use std::env;
#[cfg(test)]
use std::sync::Mutex;

/// Beach application configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// The session server address (defaults to "localhost")
    pub session_server: String,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        let server = env::var("BEACH_SESSION_SERVER")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        // Normalize localhost to IPv4 to avoid IPv6 (::1) preference on macOS
        let server = if server.starts_with("localhost:") {
            server.replacen("localhost", "127.0.0.1", 1)
        } else { server };
        Self { session_server: server }
    }

    /// Get the session server URL
    pub fn session_server_url(&self) -> String {
        // For now, just return the server address
        // In the future, this could include protocol and port
        self.session_server.clone()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self { session_server: "127.0.0.1:8080".to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;
    
    // Mutex to ensure environment variable tests don't run in parallel
    static ENV_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.session_server, "127.0.0.1:8080");
    }

    #[test]
    fn test_config_from_env_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        
        // Clear the env var to test default
        unsafe {
            env::remove_var("BEACH_SESSION_SERVER");
        }
        let config = Config::from_env();
        assert_eq!(config.session_server, "127.0.0.1:8080");
    }

    #[test]
    fn test_config_from_env_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        
        // Save current value
        let original = env::var("BEACH_SESSION_SERVER").ok();
        
        // Set custom env var
        unsafe {
            env::set_var("BEACH_SESSION_SERVER", "custom-server.example.com");
        }
        let config = Config::from_env();
        assert_eq!(config.session_server, "custom-server.example.com");
        
        // Restore original value
        unsafe {
            if let Some(orig) = original {
                env::set_var("BEACH_SESSION_SERVER", orig);
            } else {
                env::remove_var("BEACH_SESSION_SERVER");
            }
        }
    }
}

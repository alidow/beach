use anyhow::Result;
use reqwest::Client;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

#[derive(Debug, Serialize)]
struct RegisterSessionRequest {
    session_id: String,
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterSessionResponse {
    success: bool,
    session_url: String,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct JoinSessionRequest {
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JoinSessionResponse {
    success: bool,
    message: Option<String>,
    webrtc_offer: Option<serde_json::Value>,
}

pub struct SessionClient {
    client: Client,
    base_url: String,
}

impl SessionClient {
    pub fn new(session_server: &str) -> Self {
        // Normalize localhost to IPv4 to avoid IPv6 (::1) preference
        let server = if session_server.contains("localhost") {
            session_server.replace("localhost", "127.0.0.1")
        } else { session_server.to_string() };

        let base_url = if server.starts_with("http://") || server.starts_with("https://") {
            server
        } else {
            format!("http://{}", server)
        };
        
        // Build a client with conservative timeouts and no proxy to avoid
        // hanging when localhost session server is unavailable or proxied.
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client, base_url }
    }

    /// Register a new session with the session server
    pub async fn register_session(&self, session_id: &str, passphrase: Option<&str>) -> Result<String> {
        let request = RegisterSessionRequest {
            session_id: session_id.to_string(),
            passphrase: passphrase.map(|p| p.to_string()),
        };

        let response = self.client
            .post(format!("{}/sessions", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to register session: {}", response.status()));
        }

        let resp: RegisterSessionResponse = response.json().await?;
        
        if !resp.success {
            return Err(anyhow::anyhow!("Session registration failed: {}", 
                resp.message.unwrap_or_else(|| "Unknown error".to_string())));
        }

        Ok(resp.session_url)
    }

    /// Join an existing session
    pub async fn join_session(&self, session_id: &str, passphrase: Option<&str>) -> Result<()> {
        let request = JoinSessionRequest {
            passphrase: passphrase.map(|p| p.to_string()),
        };

        let response = self.client
            .post(format!("{}/sessions/{}/join", self.base_url, session_id))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to join session: {}", response.status()));
        }

        let resp: JoinSessionResponse = response.json().await?;
        
        if !resp.success {
            return Err(anyhow::anyhow!("Failed to join session: {}", 
                resp.message.unwrap_or_else(|| "Unknown error".to_string())));
        }

        // TODO: Handle WebRTC offer when implemented
        if let Some(_offer) = resp.webrtc_offer {
            eprintln!("🏖️  WebRTC signaling will be implemented in the future");
        }

        Ok(())
    }

    /// Check if a session exists
    pub async fn session_exists(&self, session_id: &str) -> Result<bool> {
        let response = self.client
            .get(format!("{}/sessions/{}", self.base_url, session_id))
            .send()
            .await?;

        if !response.status().is_success() {
            return Ok(false);
        }

        #[derive(Deserialize)]
        struct StatusResponse {
            exists: bool,
        }

        let resp: StatusResponse = response.json().await?;
        Ok(resp.exists)
    }
}

/// Hash a passphrase using SHA-256
pub fn hash_passphrase(passphrase: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    format!("{:x}", hasher.finalize())
}

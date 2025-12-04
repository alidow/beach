pub mod terminal;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    base_url: Url,
    bearer_token: Option<String>,
}

impl SessionConfig {
    pub fn new(server_base_url: impl AsRef<str>) -> Result<Self, SessionError> {
        // Allow a centralized override so callers and env stay consistent.
        let mut base = std::env::var("BEACH_SESSION_SERVER_BASE")
            .ok()
            .and_then(|s| {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
            .unwrap_or_else(|| server_base_url.as_ref().trim().to_string());
        if base.is_empty() {
            return Err(SessionError::InvalidConfig(
                "session server base url cannot be empty".into(),
            ));
        }
        if !base.contains("://") {
            let inferred_scheme = infer_scheme(&base);
            base = format!("{inferred_scheme}{base}");
        }
        let parsed = Url::parse(&base).map_err(|err| {
            SessionError::InvalidConfig(format!("invalid session server url: {err}"))
        })?;
        Ok(Self {
            base_url: parsed,
            bearer_token: None,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn with_bearer_token(mut self, token: Option<String>) -> Self {
        self.bearer_token = token;
        self
    }

    pub fn set_bearer_token(&mut self, token: Option<String>) {
        self.bearer_token = token;
    }

    pub fn bearer_token(&self) -> Option<&str> {
        self.bearer_token.as_deref()
    }
}

#[derive(Clone)]
pub struct SessionManager {
    config: Arc<SessionConfig>,
    backend: Arc<dyn SessionBackend>,
}

impl SessionManager {
    pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
        let backend = Arc::new(ReqwestSessionBackend::new()?);
        Ok(Self {
            config: Arc::new(config),
            backend,
        })
    }

    #[cfg(test)]
    fn with_backend(config: SessionConfig, backend: Arc<dyn SessionBackend>) -> Self {
        Self {
            config: Arc::new(config),
            backend,
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub async fn host(&self) -> Result<HostSession, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        let env_host_pass = std::env::var("BEACH_HOST_PASSPHRASE").ok();
        let env_smoke_pass = std::env::var("BEACH_SMOKE_PASSPHRASE").ok();
        let passphrase = env_host_pass
            .clone()
            .or(env_smoke_pass.clone())
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .or_else(|| Some("SMOKEP".to_string()));
        eprintln!(
            "host register: session_id={} passphrase_present={} host_env={:?} smoke_env={:?} chosen_pass={} session_server_base={}",
            session_id,
            passphrase.is_some(),
            env_host_pass,
            env_smoke_pass,
            passphrase.clone().unwrap_or_default(),
            self.config.base_url()
        );
        let request = RegisterSessionRequest {
            session_id: session_id.clone(),
            passphrase,
        };

        let response = self
            .backend
            .register_session(self.config.base_url(), self.config.bearer_token(), &request)
            .await?;

        let RegisterSessionResponse {
            success,
            message,
            session_id: returned_id,
            session_url: response_session_url,
            join_code,
            transports,
            websocket_url,
            transport_hints,
        } = response;
        tracing::debug!(
            target = "beach::session",
            session_id = %session_id,
            join_code = %join_code.as_deref().unwrap_or(""),
            success = success,
            "host session registered"
        );

        if !success {
            let message = message.unwrap_or_else(|| "session registration failed".to_string());
            return Err(SessionError::Server(message));
        }

        if let Some(server_session_id) = returned_id {
            if server_session_id != session_id {
                return Err(SessionError::InvalidResponse(format!(
                    "session id mismatch: expected {session_id}, got {server_session_id}"
                )));
            }
        }

        let join_code =
            join_code.ok_or_else(|| SessionError::InvalidResponse("missing join code".into()))?;
        validate_join_code(&join_code)?;

        let session_url = if let Some(ref raw) = response_session_url {
            parse_url(raw, "session_url")?
        } else {
            self.default_session_url(&session_id)?
        };

        let offers = parse_transports(transports, None, websocket_url)?;

        let handle = SessionHandle {
            role: SessionRole::Host,
            session_id,
            session_url,
            join_code: Some(join_code.clone()),
            offers,
            transport_hints,
        };

        Ok(HostSession { handle })
    }

    pub async fn join(
        &self,
        session_id: &str,
        passphrase: Option<&str>,
        viewer_token: Option<&str>,
        label: Option<&str>,
        request_mcp: bool,
    ) -> Result<JoinedSession, SessionError> {
        let cleaned_passphrase = passphrase
            .map(|code| {
                validate_join_code(code)?;
                Ok::<String, SessionError>(code.trim().to_string())
            })
            .transpose()?;

        let cleaned_viewer_token = viewer_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());

        if cleaned_passphrase.is_none() && cleaned_viewer_token.is_none() {
            return Err(SessionError::InvalidConfig(
                "passphrase or viewer token required".into(),
            ));
        }

        let request = JoinSessionRequest {
            passphrase: cleaned_passphrase.clone(),
            viewer_token: cleaned_viewer_token,
            label: label
                .map(|value| value.trim().to_string())
                .filter(|s| !s.is_empty()),
            mcp: if request_mcp { Some(true) } else { None },
        };

        let response = self
            .backend
            .join_session(
                self.config.base_url(),
                self.config.bearer_token(),
                session_id,
                &request,
            )
            .await?;

        let JoinSessionResponse {
            success,
            message,
            session_url: response_session_url,
            transports,
            webrtc_offer,
            websocket_url,
        } = response;

        if !success {
            let message = message.unwrap_or_else(|| "session join failed".to_string());
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("invalid") || lowered.contains("code") {
                return Err(SessionError::AuthenticationFailed(message));
            }
            return Err(SessionError::Server(message));
        }

        let session_url = if let Some(ref raw) = response_session_url {
            parse_url(raw, "session_url")?
        } else {
            self.default_session_url(session_id)?
        };

        let offers = parse_transports(transports, webrtc_offer, websocket_url)?;

        let handle = SessionHandle {
            role: SessionRole::Participant,
            session_id: session_id.to_string(),
            session_url,
            join_code: None,
            offers,
            transport_hints: HashMap::new(),
        };

        Ok(JoinedSession { handle })
    }

    fn default_session_url(&self, session_id: &str) -> Result<Url, SessionError> {
        self.config
            .base_url()
            .join(&format!("sessions/{session_id}"))
            .map_err(|err| {
                SessionError::InvalidConfig(format!(
                    "unable to construct session url for {session_id}: {err}"
                ))
            })
    }
}

fn infer_scheme(base: &str) -> &'static str {
    let host_part = base
        .split('/')
        .next()
        .unwrap_or(base)
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or(base);
    let host_lower = host_part.to_ascii_lowercase();
    if host_lower.starts_with("localhost")
        || host_lower == "0.0.0.0"
        || host_lower.starts_with("127.")
        || host_lower == "::1"
        || host_lower.starts_with("10.")
        || host_lower.starts_with("192.168.")
        || host_lower
            .strip_prefix("172.")
            .and_then(|rest| rest.split('.').next())
            .and_then(|octet| octet.parse::<u8>().ok())
            .map(|octet| (16..32).contains(&octet))
            .unwrap_or(false)
    {
        "http://"
    } else {
        "https://"
    }
}

#[derive(Debug, Clone)]
pub struct HostSession {
    handle: SessionHandle,
}

impl HostSession {
    pub fn session_id(&self) -> &str {
        &self.handle.session_id
    }

    pub fn join_code(&self) -> &str {
        self.handle
            .join_code
            .as_deref()
            .expect("host session must include join code")
    }

    pub fn offers(&self) -> &[TransportOffer] {
        &self.handle.offers
    }

    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    pub fn into_handle(self) -> SessionHandle {
        self.handle
    }
}

#[derive(Debug, Clone)]
pub struct JoinedSession {
    handle: SessionHandle,
}

impl JoinedSession {
    pub fn session_id(&self) -> &str {
        &self.handle.session_id
    }

    pub fn offers(&self) -> &[TransportOffer] {
        &self.handle.offers
    }

    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    pub fn into_handle(self) -> SessionHandle {
        self.handle
    }
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub role: SessionRole,
    pub session_id: String,
    pub session_url: Url,
    pub join_code: Option<String>,
    pub offers: Vec<TransportOffer>,
    pub transport_hints: HashMap<String, Value>,
}

impl SessionHandle {
    pub fn role(&self) -> SessionRole {
        self.role
    }

    pub fn join_code(&self) -> Option<&str> {
        self.join_code.as_deref()
    }

    pub fn offers(&self) -> &[TransportOffer] {
        &self.offers
    }

    pub fn transport_hints(&self) -> &HashMap<String, Value> {
        &self.transport_hints
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn session_url(&self) -> &Url {
        &self.session_url
    }

    pub fn preferred_offer(&self) -> Option<&TransportOffer> {
        self.offers.first()
    }
}

impl fmt::Display for SessionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "session={} role={:?} url={}",
            self.session_id, self.role, self.session_url
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Host,
    Participant,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransportOffer {
    WebRtc { offer: Value },
    WebSocket { url: String },
    WebSocketFallback { url: String },
    Ipc,
}

impl TransportOffer {
    pub fn label(&self) -> &'static str {
        match self {
            TransportOffer::WebRtc { .. } => "webrtc",
            TransportOffer::WebSocket { .. } => "websocket",
            TransportOffer::WebSocketFallback { .. } => "websocket_fallback",
            TransportOffer::Ipc => "ipc",
        }
    }
}

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("invalid session configuration: {0}")]
    InvalidConfig(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("unexpected http status {0}")]
    HttpStatus(StatusCode),
    #[error("server rejected request: {0}")]
    Server(String),
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("join code must be a six character alphanumeric string")]
    InvalidJoinCode,
}

#[async_trait]
trait SessionBackend: Send + Sync {
    async fn register_session(
        &self,
        base_url: &Url,
        auth_token: Option<&str>,
        request: &RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, SessionError>;

    async fn join_session(
        &self,
        base_url: &Url,
        auth_token: Option<&str>,
        session_id: &str,
        request: &JoinSessionRequest,
    ) -> Result<JoinSessionResponse, SessionError>;
}

struct ReqwestSessionBackend {
    client: reqwest::Client,
}

impl ReqwestSessionBackend {
    fn new() -> Result<Self, SessionError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .no_proxy()
            .build()?;
        Ok(Self { client })
    }
}

#[async_trait]
impl SessionBackend for ReqwestSessionBackend {
    async fn register_session(
        &self,
        base_url: &Url,
        auth_token: Option<&str>,
        request: &RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, SessionError> {
        let endpoint = base_url.join("sessions").map_err(|err| {
            SessionError::InvalidConfig(format!("invalid sessions endpoint: {err}"))
        })?;
        let mut builder = self.client.post(endpoint);
        if let Some(token) = auth_token {
            builder = builder.bearer_auth(token);
        }
        let response = builder.json(request).send().await?;
        if !response.status().is_success() {
            return Err(SessionError::HttpStatus(response.status()));
        }
        let payload = response.json::<RegisterSessionResponse>().await?;
        Ok(payload)
    }

    async fn join_session(
        &self,
        base_url: &Url,
        auth_token: Option<&str>,
        session_id: &str,
        request: &JoinSessionRequest,
    ) -> Result<JoinSessionResponse, SessionError> {
        let endpoint = base_url
            .join(&format!("sessions/{session_id}/join"))
            .map_err(|err| {
                SessionError::InvalidConfig(format!(
                    "invalid join endpoint for session {session_id}: {err}"
                ))
            })?;
        let mut builder = self.client.post(endpoint);
        if let Some(token) = auth_token {
            builder = builder.bearer_auth(token);
        }
        let response = builder.json(request).send().await?;
        if !response.status().is_success() {
            return Err(SessionError::HttpStatus(response.status()));
        }
        let payload = response.json::<JoinSessionResponse>().await?;
        Ok(payload)
    }
}

#[derive(Debug, Serialize)]
struct RegisterSessionRequest {
    session_id: String,
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegisterSessionResponse {
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    session_url: Option<String>,
    #[serde(default)]
    join_code: Option<String>,
    #[serde(default)]
    transports: Vec<AdvertisedTransport>,
    #[serde(default)]
    websocket_url: Option<String>,
    #[serde(default)]
    transport_hints: HashMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct JoinSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct JoinSessionResponse {
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    session_url: Option<String>,
    #[serde(default)]
    transports: Vec<AdvertisedTransport>,
    #[serde(default)]
    webrtc_offer: Option<Value>,
    #[serde(default)]
    websocket_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AdvertisedTransport {
    kind: AdvertisedTransportKind,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdvertisedTransportKind {
    WebRtc,
    WebSocket,
    Ipc,
}

fn parse_transports(
    transports: Vec<AdvertisedTransport>,
    fallback_webrtc: Option<Value>,
    fallback_websocket: Option<String>,
) -> Result<Vec<TransportOffer>, SessionError> {
    let mut offers = Vec::new();
    for advert in transports {
        match advert.kind {
            AdvertisedTransportKind::WebRtc => {
                if let Some(metadata) = advert.metadata {
                    offers.push(TransportOffer::WebRtc { offer: metadata });
                } else if let Some(url) = advert.url {
                    offers.push(TransportOffer::WebRtc {
                        offer: json!({ "url": url }),
                    });
                }
            }
            AdvertisedTransportKind::WebSocket => {
                if let Some(url) = advert.url {
                    let trimmed = url.trim();
                    if !trimmed.is_empty() {
                        offers.push(TransportOffer::WebSocket {
                            url: trimmed.to_string(),
                        });
                    }
                }
            }
            AdvertisedTransportKind::Ipc => {
                offers.push(TransportOffer::Ipc);
            }
        }
    }

    let has_webrtc = offers
        .iter()
        .any(|offer| matches!(offer, TransportOffer::WebRtc { .. }));
    if !has_webrtc {
        if let Some(value) = fallback_webrtc {
            offers.push(TransportOffer::WebRtc { offer: value });
        }
    }

    let has_websocket = offers.iter().any(|offer| {
        matches!(
            offer,
            TransportOffer::WebSocket { .. } | TransportOffer::WebSocketFallback { .. }
        )
    });
    if !has_websocket {
        if let Some(url) = fallback_websocket {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                offers.push(TransportOffer::WebSocketFallback {
                    url: trimmed.to_string(),
                });
            }
        }
    }

    if offers.is_empty() {
        offers.push(TransportOffer::Ipc);
    }

    Ok(offers)
}

fn validate_join_code(code: &str) -> Result<(), SessionError> {
    if code.len() == 6 && code.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(SessionError::InvalidJoinCode)
    }
}

fn parse_url(raw: &str, field: &str) -> Result<Url, SessionError> {
    Url::parse(raw).map_err(|err| {
        SessionError::InvalidResponse(format!("{field} contains invalid url '{raw}': {err}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn defaults_to_https_for_public_hosts() {
        assert_eq!(infer_scheme("api.beach.sh"), "https://");
        assert_eq!(infer_scheme("beach.sh/some/path"), "https://");
        assert_eq!(infer_scheme("13.215.162.4"), "https://");
    }

    #[test]
    fn defaults_to_http_for_local_hosts() {
        for host in [
            "localhost",
            "localhost:4132",
            "127.0.0.1",
            "127.0.0.1:8080",
            "0.0.0.0",
            "10.0.0.5",
            "192.168.1.10",
            "172.16.0.1",
            "172.31.255.255",
            "[::1]",
        ] {
            assert_eq!(infer_scheme(host), "http://");
        }
    }

    #[test]
    fn session_config_infers_scheme() {
        let https = SessionConfig::new("api.beach.sh").unwrap();
        assert_eq!(https.base_url().as_str(), "https://api.beach.sh/");

        let http = SessionConfig::new("localhost:8080").unwrap();
        assert_eq!(http.base_url().as_str(), "http://localhost:8080/");
    }

    #[derive(Clone)]
    struct MockSessionBackend {
        sessions: Arc<Mutex<HashMap<String, String>>>,
        last_token: Arc<Mutex<Option<String>>>,
    }

    impl MockSessionBackend {
        fn new() -> Self {
            Self {
                sessions: Arc::new(Mutex::new(HashMap::new())),
                last_token: Arc::new(Mutex::new(None)),
            }
        }

        async fn last_token(&self) -> Option<String> {
            self.last_token.lock().await.clone()
        }
    }

    #[async_trait]
    impl SessionBackend for MockSessionBackend {
        async fn register_session(
            &self,
            _base_url: &Url,
            auth_token: Option<&str>,
            request: &RegisterSessionRequest,
        ) -> Result<RegisterSessionResponse, SessionError> {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&request.session_id) {
                return Ok(RegisterSessionResponse {
                    success: false,
                    message: Some("session already exists".into()),
                    session_id: Some(request.session_id.clone()),
                    session_url: Some({
                        let session_id = &request.session_id;
                        format!("http://mock/{session_id}")
                    }),
                    join_code: None,
                    transports: Vec::new(),
                    websocket_url: None,
                    transport_hints: HashMap::new(),
                });
            }

            let code = "654321".to_string();
            sessions.insert(request.session_id.clone(), code.clone());

            {
                let mut slot = self.last_token.lock().await;
                *slot = auth_token.map(|token| token.to_string());
            }

            Ok(RegisterSessionResponse {
                success: true,
                message: None,
                session_id: Some(request.session_id.clone()),
                session_url: Some({
                    let session_id = &request.session_id;
                    format!("http://mock/{session_id}")
                }),
                join_code: Some(code),
                transports: vec![AdvertisedTransport {
                    kind: AdvertisedTransportKind::WebSocket,
                    url: Some("ws://mock/signal".into()),
                    metadata: None,
                }],
                websocket_url: None,
                transport_hints: HashMap::new(),
            })
        }

        async fn join_session(
            &self,
            _base_url: &Url,
            auth_token: Option<&str>,
            session_id: &str,
            request: &JoinSessionRequest,
        ) -> Result<JoinSessionResponse, SessionError> {
            let sessions = self.sessions.lock().await;
            match sessions.get(session_id) {
                Some(expected) => {
                    let passphrase_valid = request
                        .passphrase
                        .as_ref()
                        .map(|value| value == expected)
                        .unwrap_or(false);
                    let viewer_token_valid = request
                        .viewer_token
                        .as_ref()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false);
                    if !(passphrase_valid || viewer_token_valid) {
                        return Ok(JoinSessionResponse {
                            success: false,
                            message: Some("invalid code".into()),
                            session_url: None,
                            transports: Vec::new(),
                            webrtc_offer: None,
                            websocket_url: None,
                        });
                    }

                    {
                        let mut slot = self.last_token.lock().await;
                        *slot = auth_token.map(|token| token.to_string());
                    }
                    Ok(JoinSessionResponse {
                        success: true,
                        message: None,
                        session_url: Some(format!("http://mock/{session_id}")),
                        transports: vec![
                            AdvertisedTransport {
                                kind: AdvertisedTransportKind::WebRtc,
                                url: None,
                                metadata: Some(json!({ "type": "offer", "sdp": "mock" })),
                            },
                            AdvertisedTransport {
                                kind: AdvertisedTransportKind::WebSocket,
                                url: Some("ws://mock/relay".into()),
                                metadata: None,
                            },
                        ],
                        webrtc_offer: Some(json!({ "type": "offer", "sdp": "mock" })),
                        websocket_url: None,
                    })
                }
                None => Ok(JoinSessionResponse {
                    success: false,
                    message: Some("session not found".into()),
                    session_url: None,
                    transports: Vec::new(),
                    webrtc_offer: None,
                    websocket_url: None,
                }),
            }
        }
    }

    #[test]
    fn validate_join_code_accepts_alphanumeric() {
        assert!(validate_join_code("A1B2C3").is_ok());
        assert!(validate_join_code("123456").is_ok());
    }

    #[test]
    fn validate_join_code_rejects_invalid_codes() {
        assert!(validate_join_code("ABC12!").is_err());
        assert!(validate_join_code("ABCDE").is_err());
        assert!(validate_join_code("ABCDEFG").is_err());
    }

    #[test_timeout::tokio_timeout_test]
    async fn host_session_returns_join_code_and_offers() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend);

        let hosted = manager.host().await.unwrap();
        assert_eq!(hosted.join_code(), "654321");
        assert_eq!(hosted.offers().len(), 1);
        assert!(matches!(
            hosted.offers()[0],
            TransportOffer::WebSocket { ref url } if url == "ws://mock/signal"
        ));
    }

    #[test_timeout::tokio_timeout_test]
    async fn join_session_with_valid_code_yields_webrtc_offer() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend.clone());

        let hosted = manager.host().await.unwrap();
        let joiner = manager
            .join(
                hosted.session_id(),
                Some(hosted.join_code()),
                None,
                None,
                false,
            )
            .await
            .unwrap();

        assert!(
            joiner
                .offers()
                .iter()
                .any(|offer| matches!(offer, TransportOffer::WebRtc { .. }))
        );
        assert!(joiner.offers().iter().any(|offer| matches!(
            offer,
            TransportOffer::WebSocket { .. } | TransportOffer::WebSocketFallback { .. }
        )));
    }

    #[test_timeout::tokio_timeout_test]
    async fn join_session_with_invalid_code_fails() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend);

        let hosted = manager.host().await.unwrap();
        let err = manager
            .join(hosted.session_id(), Some("000000"), None, None, false)
            .await
            .unwrap_err();

        assert!(matches!(err, SessionError::AuthenticationFailed(_)));
    }

    #[test_timeout::tokio_timeout_test]
    async fn join_session_with_viewer_token_requires_no_passphrase() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend);

        let hosted = manager.host().await.unwrap();
        let joiner = manager
            .join(hosted.session_id(), None, Some("viewer-token"), None, false)
            .await
            .unwrap();

        assert!(
            joiner
                .offers()
                .iter()
                .any(|offer| matches!(offer, TransportOffer::WebRtc { .. }))
        );
    }

    #[test_timeout::tokio_timeout_test]
    async fn join_session_rejects_missing_credentials() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend);

        let err = manager
            .join("session-123", None, None, None, false)
            .await
            .unwrap_err();

        assert!(matches!(err, SessionError::InvalidConfig(_)));
    }

    #[test_timeout::tokio_timeout_test]
    async fn register_session_passes_bearer_token_to_backend() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server")
            .unwrap()
            .with_bearer_token(Some("token-123".into()));
        let manager = SessionManager::with_backend(config, backend.clone());

        let _ = manager.host().await.unwrap();

        let observed = backend.last_token().await;
        assert_eq!(observed.as_deref(), Some("token-123"));
    }
}

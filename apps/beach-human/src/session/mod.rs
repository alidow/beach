use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    base_url: Url,
}

impl SessionConfig {
    pub fn new(server_base_url: impl AsRef<str>) -> Result<Self, SessionError> {
        let mut base = server_base_url.as_ref().trim().to_string();
        if base.is_empty() {
            return Err(SessionError::InvalidConfig(
                "session server base url cannot be empty".into(),
            ));
        }
        if !base.starts_with("http://") && !base.starts_with("https://") {
            base = format!("http://{}", base);
        }
        let parsed = Url::parse(&base).map_err(|err| {
            SessionError::InvalidConfig(format!("invalid session server url: {err}"))
        })?;
        Ok(Self { base_url: parsed })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
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
        let request = RegisterSessionRequest {
            session_id: session_id.clone(),
            passphrase: None,
        };

        let response = self
            .backend
            .register_session(self.config.base_url(), &request)
            .await?;

        let RegisterSessionResponse {
            success,
            message,
            session_id: returned_id,
            session_url: response_session_url,
            join_code,
            transports,
            websocket_url,
        } = response;

        if !success {
            let message = message.unwrap_or_else(|| "session registration failed".to_string());
            return Err(SessionError::Server(message));
        }

        if let Some(server_session_id) = returned_id {
            if server_session_id != session_id {
                return Err(SessionError::InvalidResponse(format!(
                    "session id mismatch: expected {}, got {}",
                    session_id, server_session_id
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
        };

        Ok(HostSession { handle })
    }

    pub async fn join(
        &self,
        session_id: &str,
        join_code: &str,
    ) -> Result<JoinedSession, SessionError> {
        validate_join_code(join_code)?;

        let request = JoinSessionRequest {
            passphrase: join_code.to_string(),
        };

        let response = self
            .backend
            .join_session(self.config.base_url(), session_id, &request)
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
        };

        Ok(JoinedSession { handle })
    }

    fn default_session_url(&self, session_id: &str) -> Result<Url, SessionError> {
        self.config
            .base_url()
            .join(&format!("sessions/{}", session_id))
            .map_err(|err| {
                SessionError::InvalidConfig(format!(
                    "unable to construct session url for {session_id}: {err}"
                ))
            })
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
    Ipc,
}

impl TransportOffer {
    pub fn label(&self) -> &'static str {
        match self {
            TransportOffer::WebRtc { .. } => "webrtc",
            TransportOffer::WebSocket { .. } => "websocket",
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
    #[error("join code must be six numeric digits")]
    InvalidJoinCode,
}

#[async_trait]
trait SessionBackend: Send + Sync {
    async fn register_session(
        &self,
        base_url: &Url,
        request: &RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, SessionError>;

    async fn join_session(
        &self,
        base_url: &Url,
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
        request: &RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, SessionError> {
        let endpoint = base_url.join("sessions").map_err(|err| {
            SessionError::InvalidConfig(format!("invalid sessions endpoint: {err}"))
        })?;
        let response = self.client.post(endpoint).json(request).send().await?;
        if !response.status().is_success() {
            return Err(SessionError::HttpStatus(response.status()));
        }
        let payload = response.json::<RegisterSessionResponse>().await?;
        Ok(payload)
    }

    async fn join_session(
        &self,
        base_url: &Url,
        session_id: &str,
        request: &JoinSessionRequest,
    ) -> Result<JoinSessionResponse, SessionError> {
        let endpoint = base_url
            .join(&format!("sessions/{}/join", session_id))
            .map_err(|err| {
                SessionError::InvalidConfig(format!(
                    "invalid join endpoint for session {session_id}: {err}"
                ))
            })?;
        let response = self.client.post(endpoint).json(request).send().await?;
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
}

#[derive(Debug, Serialize)]
struct JoinSessionRequest {
    passphrase: String,
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

    let has_websocket = offers
        .iter()
        .any(|offer| matches!(offer, TransportOffer::WebSocket { .. }));
    if !has_websocket {
        if let Some(url) = fallback_websocket {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                offers.push(TransportOffer::WebSocket {
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
    if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
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

    #[derive(Clone)]
    struct MockSessionBackend {
        sessions: Arc<Mutex<HashMap<String, String>>>,
    }

    impl MockSessionBackend {
        fn new() -> Self {
            Self {
                sessions: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl SessionBackend for MockSessionBackend {
        async fn register_session(
            &self,
            _base_url: &Url,
            request: &RegisterSessionRequest,
        ) -> Result<RegisterSessionResponse, SessionError> {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(&request.session_id) {
                return Ok(RegisterSessionResponse {
                    success: false,
                    message: Some("session already exists".into()),
                    session_id: Some(request.session_id.clone()),
                    session_url: Some(format!("http://mock/{}", request.session_id)),
                    join_code: None,
                    transports: Vec::new(),
                    websocket_url: None,
                });
            }

            let code = "654321".to_string();
            sessions.insert(request.session_id.clone(), code.clone());

            Ok(RegisterSessionResponse {
                success: true,
                message: None,
                session_id: Some(request.session_id.clone()),
                session_url: Some(format!("http://mock/{}", request.session_id)),
                join_code: Some(code),
                transports: vec![AdvertisedTransport {
                    kind: AdvertisedTransportKind::WebSocket,
                    url: Some("ws://mock/signal".into()),
                    metadata: None,
                }],
                websocket_url: None,
            })
        }

        async fn join_session(
            &self,
            _base_url: &Url,
            session_id: &str,
            request: &JoinSessionRequest,
        ) -> Result<JoinSessionResponse, SessionError> {
            let sessions = self.sessions.lock().await;
            match sessions.get(session_id) {
                Some(expected) if *expected == request.passphrase => Ok(JoinSessionResponse {
                    success: true,
                    message: None,
                    session_url: Some(format!("http://mock/{}", session_id)),
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
                }),
                Some(_) => Ok(JoinSessionResponse {
                    success: false,
                    message: Some("invalid code".into()),
                    session_url: None,
                    transports: Vec::new(),
                    webrtc_offer: None,
                    websocket_url: None,
                }),
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

    #[tokio::test]
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

    #[tokio::test]
    async fn join_session_with_valid_code_yields_webrtc_offer() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend.clone());

        let hosted = manager.host().await.unwrap();
        let joiner = manager
            .join(hosted.session_id(), hosted.join_code())
            .await
            .unwrap();

        assert!(
            joiner
                .offers()
                .iter()
                .any(|offer| matches!(offer, TransportOffer::WebRtc { .. }))
        );
        assert!(
            joiner
                .offers()
                .iter()
                .any(|offer| matches!(offer, TransportOffer::WebSocket { .. }))
        );
    }

    #[tokio::test]
    async fn join_session_with_invalid_code_fails() {
        let backend = Arc::new(MockSessionBackend::new());
        let config = SessionConfig::new("http://mock.server").unwrap();
        let manager = SessionManager::with_backend(config, backend);

        let hosted = manager.host().await.unwrap();
        let err = manager
            .join(hosted.session_id(), "000000")
            .await
            .unwrap_err();

        assert!(matches!(err, SessionError::AuthenticationFailed(_)));
    }
}

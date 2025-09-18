use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::{
    session::{hash_passphrase, verify_passphrase},
    storage::{SessionInfo, Storage},
};

pub type SharedStorage = Arc<Mutex<Storage>>;

#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum AdvertisedTransportKind {
    WebRtc,
    WebSocket,
    Ipc,
}

#[derive(Debug, Serialize, Clone)]
pub struct AdvertisedTransport {
    kind: AdvertisedTransportKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

impl AdvertisedTransport {
    fn webrtc(metadata: serde_json::Value) -> Self {
        Self {
            kind: AdvertisedTransportKind::WebRtc,
            url: None,
            metadata: Some(metadata),
        }
    }

    fn websocket(url: String) -> Self {
        Self {
            kind: AdvertisedTransportKind::WebSocket,
            url: Some(url),
            metadata: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterSessionRequest {
    pub session_id: String,
    pub passphrase: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterSessionResponse {
    pub success: bool,
    pub session_url: String,
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_code: Option<String>,
    #[serde(default)]
    pub transports: Vec<AdvertisedTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JoinSessionRequest {
    pub passphrase: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinSessionResponse {
    pub success: bool,
    pub message: Option<String>,
    pub webrtc_offer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_url: Option<String>,
    #[serde(default)]
    pub transports: Vec<AdvertisedTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub websocket_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionStatusResponse {
    pub exists: bool,
    pub created_at: Option<u64>,
}

fn generate_join_code() -> String {
    let mut rng = rand::thread_rng();
    format!("{:06}", rng.gen_range(0..=999_999))
}

fn normalize_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{}", trimmed)
    }
}

fn websocket_url(base_http: &str, session_id: &str) -> String {
    let ws_base = if let Some(rest) = base_http.strip_prefix("https://") {
        format!("wss://{}", rest)
    } else if let Some(rest) = base_http.strip_prefix("http://") {
        format!("ws://{}", rest)
    } else {
        format!("ws://{}", base_http)
    };
    format!("{}/ws/{}", ws_base.trim_end_matches('/'), session_id)
}

fn signaling_url(base_http: &str, session_id: &str) -> String {
    format!(
        "{}/sessions/{}/webrtc",
        base_http.trim_end_matches('/'),
        session_id
    )
}

/// POST /sessions - Register a new session
pub async fn register_session(
    State(storage): State<SharedStorage>,
    Json(payload): Json<RegisterSessionRequest>,
) -> Result<Json<RegisterSessionResponse>, StatusCode> {
    debug!("Registering session: {}", payload.session_id);

    let mut storage = storage.lock().await;

    // Check if session already exists
    match storage.session_exists(&payload.session_id).await {
        Ok(true) => {
            return Ok(Json(RegisterSessionResponse {
                success: false,
                session_url: String::new(),
                message: Some("Session already exists".to_string()),
                session_id: None,
                join_code: None,
                transports: Vec::new(),
                websocket_url: None,
            }));
        }
        Ok(false) => {}
        Err(e) => {
            error!("Failed to check session existence: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    // Hash the passphrase if provided
    let supplied_passphrase = payload.passphrase.clone().filter(|p| !p.trim().is_empty());

    let join_code_plain = supplied_passphrase.unwrap_or_else(generate_join_code);
    let passphrase_hash = hash_passphrase(&join_code_plain);

    let session_server_env = std::env::var("BEACH_SESSION_SERVER")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let base_http = normalize_base_url(&session_server_env);

    let mut session = SessionInfo::new(
        payload.session_id.clone(),
        passphrase_hash,
        join_code_plain.clone(),
    );
    session.server_address = Some(base_http.clone());

    match storage.register_session(session).await {
        Ok(_) => {
            debug!("Session {} registered successfully", payload.session_id);

            let session_url = format!(
                "{}/sessions/{}",
                base_http.trim_end_matches('/'),
                payload.session_id
            );
            let websocket_url = websocket_url(&base_http, &payload.session_id);
            let signal_url = signaling_url(&base_http, &payload.session_id);
            let transports = vec![
                AdvertisedTransport::webrtc(json!({
                    "signaling_url": signal_url,
                    "role": "offerer",
                    "poll_interval_ms": 250u64,
                })),
                AdvertisedTransport::websocket(websocket_url.clone()),
            ];

            Ok(Json(RegisterSessionResponse {
                success: true,
                session_url,
                message: None,
                session_id: Some(payload.session_id.clone()),
                join_code: Some(join_code_plain),
                transports,
                websocket_url: Some(websocket_url),
            }))
        }
        Err(e) => {
            error!("Failed to register session: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// POST /sessions/{id}/join - Join an existing session
pub async fn join_session(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Json(payload): Json<JoinSessionRequest>,
) -> Result<Json<JoinSessionResponse>, StatusCode> {
    debug!("Client attempting to join session: {}", session_id);

    let mut storage = storage.lock().await;

    match storage.get_session(&session_id).await {
        Ok(Some(session)) => {
            // Verify passphrase if the session has one
            if !session.passphrase_hash.is_empty() {
                if let Some(passphrase) = payload.passphrase {
                    if !verify_passphrase(&passphrase, &session.passphrase_hash) {
                        return Ok(Json(JoinSessionResponse {
                            success: false,
                            message: Some("Invalid passphrase".to_string()),
                            webrtc_offer: None,
                            session_url: None,
                            transports: Vec::new(),
                            websocket_url: None,
                        }));
                    }
                } else {
                    return Ok(Json(JoinSessionResponse {
                        success: false,
                        message: Some("Passphrase required".to_string()),
                        webrtc_offer: None,
                        session_url: None,
                        transports: Vec::new(),
                        websocket_url: None,
                    }));
                }
            }

            // Update session TTL
            let _ = storage.update_session_ttl(&session_id).await;

            debug!("Client successfully joined session: {}", session_id);

            let base_http = session.server_address.clone().unwrap_or_else(|| {
                let env = std::env::var("BEACH_SESSION_SERVER")
                    .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
                normalize_base_url(&env)
            });

            let session_url = format!(
                "{}/sessions/{}",
                base_http.trim_end_matches('/'),
                session_id
            );
            let websocket_url = websocket_url(&base_http, &session_id);
            let signal_url = signaling_url(&base_http, &session_id);

            let transport_metadata = json!({
                "signaling_url": signal_url,
                "role": "answerer",
                "poll_interval_ms": 250u64,
            });

            let transports = vec![
                AdvertisedTransport::webrtc(transport_metadata.clone()),
                AdvertisedTransport::websocket(websocket_url.clone()),
            ];

            Ok(Json(JoinSessionResponse {
                success: true,
                message: None,
                webrtc_offer: Some(transport_metadata),
                session_url: Some(session_url),
                transports,
                websocket_url: Some(websocket_url),
            }))
        }
        Ok(None) => Ok(Json(JoinSessionResponse {
            success: false,
            message: Some("Session not found".to_string()),
            webrtc_offer: None,
            session_url: None,
            transports: Vec::new(),
            websocket_url: None,
        })),
        Err(e) => {
            error!("Failed to get session: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WebRtcSdpPayload {
    pub sdp: String,
    #[serde(rename = "type")]
    pub typ: String,
}

pub async fn post_webrtc_offer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Json(payload): Json<WebRtcSdpPayload>,
) -> Result<StatusCode, StatusCode> {
    let mut storage = storage.lock().await;
    if !storage
        .session_exists(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::NOT_FOUND);
    }

    let serialized =
        serde_json::to_string(&payload).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    storage
        .set_webrtc_offer(&session_id, &serialized)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Any previous answer is no longer relevant once a new offer is posted.
    let _ = storage.clear_webrtc_answer(&session_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_webrtc_offer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
) -> Result<Json<WebRtcSdpPayload>, StatusCode> {
    let mut storage = storage.lock().await;
    match storage
        .get_webrtc_offer(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(value) => {
            let payload: WebRtcSdpPayload =
                serde_json::from_str(&value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(Json(payload))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn post_webrtc_answer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Json(payload): Json<WebRtcSdpPayload>,
) -> Result<StatusCode, StatusCode> {
    let mut storage = storage.lock().await;
    if !storage
        .session_exists(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::NOT_FOUND);
    }

    let serialized =
        serde_json::to_string(&payload).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    storage
        .set_webrtc_answer(&session_id, &serialized)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_webrtc_answer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
) -> Result<Json<WebRtcSdpPayload>, StatusCode> {
    let mut storage = storage.lock().await;
    match storage
        .get_webrtc_answer(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(value) => {
            let payload: WebRtcSdpPayload =
                serde_json::from_str(&value).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            // Clear answer after it has been consumed.
            let _ = storage.clear_webrtc_answer(&session_id).await;
            Ok(Json(payload))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// GET /sessions/{id} - Check if session exists
pub async fn get_session_status(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionStatusResponse>, StatusCode> {
    let mut storage = storage.lock().await;

    match storage.get_session(&session_id).await {
        Ok(Some(session)) => Ok(Json(SessionStatusResponse {
            exists: true,
            created_at: Some(session.created_at),
        })),
        Ok(None) => Ok(Json(SessionStatusResponse {
            exists: false,
            created_at: None,
        })),
        Err(e) => {
            error!("Failed to get session status: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// GET /health - Health check endpoint
pub async fn health_check() -> &'static str {
    "OK"
}

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::{
    session::{hash_passphrase, verify_passphrase},
    storage::{SessionInfo, Storage},
};

pub type SharedStorage = Arc<Mutex<Storage>>;

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
}

#[derive(Debug, Deserialize)]
pub struct JoinSessionRequest {
    pub passphrase: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinSessionResponse {
    pub success: bool,
    pub message: Option<String>,
    pub webrtc_offer: Option<serde_json::Value>, // Placeholder for future WebRTC
}

#[derive(Debug, Serialize)]
pub struct SessionStatusResponse {
    pub exists: bool,
    pub created_at: Option<u64>,
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
            }));
        }
        Ok(false) => {}
        Err(e) => {
            error!("Failed to check session existence: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    // Hash the passphrase if provided
    let passphrase_hash = payload
        .passphrase
        .as_ref()
        .map(|p| hash_passphrase(p))
        .unwrap_or_default();

    let session = SessionInfo::new(payload.session_id.clone(), passphrase_hash);

    match storage.register_session(session).await {
        Ok(_) => {
            debug!("Session {} registered successfully", payload.session_id);
            
            // Get the session server from environment or use default
            let session_server = std::env::var("BEACH_SESSION_SERVER")
                .unwrap_or_else(|_| "localhost:8080".to_string());
            
            Ok(Json(RegisterSessionResponse {
                success: true,
                session_url: format!("{}/{}", session_server, payload.session_id),
                message: None,
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
                        }));
                    }
                } else {
                    return Ok(Json(JoinSessionResponse {
                        success: false,
                        message: Some("Passphrase required".to_string()),
                        webrtc_offer: None,
                    }));
                }
            }

            // Update session TTL
            let _ = storage.update_session_ttl(&session_id).await;

            debug!("Client successfully joined session: {}", session_id);
            Ok(Json(JoinSessionResponse {
                success: true,
                message: None,
                webrtc_offer: Some(serde_json::json!({
                    "placeholder": "WebRTC offer will go here"
                })),
            }))
        }
        Ok(None) => {
            Ok(Json(JoinSessionResponse {
                success: false,
                message: Some("Session not found".to_string()),
                webrtc_offer: None,
            }))
        }
        Err(e) => {
            error!("Failed to get session: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// GET /sessions/{id} - Check if session exists
pub async fn get_session_status(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionStatusResponse>, StatusCode> {
    let mut storage = storage.lock().await;

    match storage.get_session(&session_id).await {
        Ok(Some(session)) => {
            Ok(Json(SessionStatusResponse {
                exists: true,
                created_at: Some(session.created_at),
            }))
        }
        Ok(None) => {
            Ok(Json(SessionStatusResponse {
                exists: false,
                created_at: None,
            }))
        }
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
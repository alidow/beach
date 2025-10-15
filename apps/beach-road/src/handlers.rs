use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use beach_rescue_core::{
    guardrail::SoftGuardrailState, is_telemetry_enabled, CohortId, FallbackTokenClaims,
    TelemetryPreference,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::sync::Arc;
use time::Duration;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::{
    session::{hash_passphrase, verify_passphrase},
    signaling::WebRtcSdpPayload,
    storage::{SessionInfo, Storage},
};

pub type SharedStorage = Arc<Storage>;

#[derive(Clone)]
pub struct FallbackContext {
    pub storage: SharedStorage,
    pub guardrail_threshold: f64,
    pub token_ttl_seconds: u64,
    pub require_oidc: bool,
    pub paused: bool,
}

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
    #[serde(default)]
    pub mcp: bool,
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

#[derive(Debug, Serialize)]
struct FallbackTokenErrorBody {
    success: bool,
    reason: &'static str,
}

pub struct FallbackTokenErrorResponse {
    status: StatusCode,
    body: Option<FallbackTokenErrorBody>,
}

impl FallbackTokenErrorResponse {
    fn paused() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            body: Some(FallbackTokenErrorBody {
                success: false,
                reason: "fallback_paused",
            }),
        }
    }

    fn with_reason(status: StatusCode, reason: &'static str) -> Self {
        Self {
            status,
            body: Some(FallbackTokenErrorBody {
                success: false,
                reason,
            }),
        }
    }

    fn status(status: StatusCode) -> Self {
        Self { status, body: None }
    }
}

impl IntoResponse for FallbackTokenErrorResponse {
    fn into_response(self) -> Response {
        match self.body {
            Some(body) => (self.status, Json(body)).into_response(),
            None => self.status.into_response(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    status: &'static str,
    fallback_paused: bool,
}

#[derive(Debug, Deserialize)]
pub struct FallbackTokenRequest {
    pub session_id: String,
    pub cohort_id: Option<String>,
    #[serde(default)]
    pub telemetry_opt_in: bool,
    #[serde(default)]
    pub total_sessions_hint: Option<u64>,
    #[serde(default)]
    pub entitlement_proof: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FallbackTokenResponse {
    pub token: String,
    pub expires_at: time::OffsetDateTime,
    pub guardrail_ratio: f64,
    pub guardrail_soft_breach: bool,
    pub telemetry_enabled: bool,
}

fn generate_join_code() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .map(|c| char::from(c).to_ascii_uppercase())
        .take(6)
        .collect()
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

/// POST /fallback/token - Issue a fallback token for WebSocket rescue path.
pub async fn issue_fallback_token(
    State(ctx): State<FallbackContext>,
    Json(payload): Json<FallbackTokenRequest>,
) -> Result<Json<FallbackTokenResponse>, FallbackTokenErrorResponse> {
    if ctx.paused {
        warn!("fallback token minting paused; rejecting request");
        return Err(FallbackTokenErrorResponse::paused());
    }

    if ctx.require_oidc && payload.entitlement_proof.is_none() {
        return Err(FallbackTokenErrorResponse::with_reason(
            StatusCode::UNAUTHORIZED,
            "entitlement_required",
        ));
    }

    let session_id = Uuid::parse_str(&payload.session_id).map_err(|_| {
        FallbackTokenErrorResponse::with_reason(StatusCode::BAD_REQUEST, "invalid_session_id")
    })?;
    let cohort_id = payload.cohort_id.as_deref().unwrap_or("public").to_string();

    let snapshot = ctx
        .storage
        .track_fallback_activation(&cohort_id, payload.total_sessions_hint)
        .await
        .map_err(|err| {
            error!("redis guardrail error: {err:?}");
            FallbackTokenErrorResponse::status(StatusCode::INTERNAL_SERVER_ERROR)
        })?;

    let guardrail_soft_breach = matches!(
        snapshot.soft_state(ctx.guardrail_threshold),
        SoftGuardrailState::Breaching
    );
    let telemetry_pref = if payload.telemetry_opt_in {
        TelemetryPreference::Enabled
    } else {
        TelemetryPreference::Disabled
    };

    let ttl = Duration::seconds(ctx.token_ttl_seconds as i64);
    let claims = FallbackTokenClaims::new(
        session_id,
        CohortId::from(cohort_id.clone()),
        ttl,
        telemetry_pref,
    );

    let serialized = serde_json::to_vec(&claims).map_err(|err| {
        error!("failed to serialize fallback token claims: {err:?}");
        FallbackTokenErrorResponse::status(StatusCode::INTERNAL_SERVER_ERROR)
    })?;
    let token = STANDARD_NO_PAD.encode(serialized);

    Ok(Json(FallbackTokenResponse {
        token,
        expires_at: claims.expires_at,
        guardrail_ratio: snapshot.counters.fallback_ratio(),
        guardrail_soft_breach,
        telemetry_enabled: is_telemetry_enabled(telemetry_pref),
    }))
}

/// POST /sessions - Register a new session
pub async fn register_session(
    State(storage): State<SharedStorage>,
    Json(payload): Json<RegisterSessionRequest>,
) -> Result<Json<RegisterSessionResponse>, StatusCode> {
    debug!("Registering session: {}", payload.session_id);

    let storage = (*storage).clone();

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
        .unwrap_or_else(|_| "https://api.beach.sh".to_string());
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

    let storage = (*storage).clone();

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
                    .unwrap_or_else(|_| "https://api.beach.sh".to_string());
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
        Ok(None) => {
            debug!("Join attempt for missing session: {}", session_id);
            Ok(Json(JoinSessionResponse {
                success: false,
                message: Some("Session not found".to_string()),
                webrtc_offer: None,
                session_url: None,
                transports: Vec::new(),
                websocket_url: None,
            }))
        }
        Err(e) => {
            error!("Failed to get session: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OfferQuery {
    pub peer_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AnswerQuery {
    pub handshake_id: String,
}

pub async fn post_webrtc_offer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Json(payload): Json<WebRtcSdpPayload>,
) -> Result<StatusCode, StatusCode> {
    if payload.handshake_id.trim().is_empty()
        || payload.from_peer.trim().is_empty()
        || payload.to_peer.trim().is_empty()
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let storage = (*storage).clone();
    if !storage
        .session_exists(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::NOT_FOUND);
    }

    storage
        .push_webrtc_offer(&session_id, &payload)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Activity observed: refresh session TTL
    let _ = storage.update_session_ttl(&session_id).await;

    debug!(
        session = %session_id,
        %payload.handshake_id,
        %payload.from_peer,
        %payload.to_peer,
        "stored webrtc offer"
    );

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_webrtc_offer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Query(params): Query<OfferQuery>,
) -> Result<Json<WebRtcSdpPayload>, StatusCode> {
    let storage = (*storage).clone();
    match storage
        .pop_webrtc_offer_for_peer(&session_id, &params.peer_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(payload) => {
            // Activity observed: refresh session TTL
            let _ = storage.update_session_ttl(&session_id).await;
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
    if payload.handshake_id.trim().is_empty()
        || payload.from_peer.trim().is_empty()
        || payload.to_peer.trim().is_empty()
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let storage = (*storage).clone();
    if !storage
        .session_exists(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Ok(StatusCode::NOT_FOUND);
    }

    storage
        .store_webrtc_answer(&session_id, &payload)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    debug!(
        session = %session_id,
        %payload.handshake_id,
        %payload.from_peer,
        %payload.to_peer,
        "stored webrtc answer"
    );
    // Activity observed: refresh session TTL
    let _ = storage.update_session_ttl(&session_id).await;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_webrtc_answer(
    State(storage): State<SharedStorage>,
    Path(session_id): Path<String>,
    Query(params): Query<AnswerQuery>,
) -> Result<Json<WebRtcSdpPayload>, StatusCode> {
    let storage = (*storage).clone();
    match storage
        .take_webrtc_answer(&session_id, &params.handshake_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(payload) => {
            // Activity observed: refresh session TTL
            let _ = storage.update_session_ttl(&session_id).await;
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
    let storage = (*storage).clone();

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
pub async fn health_check() -> Json<HealthStatus> {
    Json(HealthStatus {
        status: "ok",
        fallback_paused: fallback_paused_env(),
    })
}

fn fallback_paused_env() -> bool {
    env::var("FALLBACK_WS_PAUSED")
        .map(|value| matches_truthy(&value))
        .unwrap_or(false)
}

fn matches_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

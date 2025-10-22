use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::state::{AppState, StateError, ViewerTokenError};

use super::{sessions::ensure_scope, ApiError, ApiResult, AuthToken};

#[derive(Debug, Deserialize)]
pub struct CreateBeachRequest {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBeachRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct BeachSummary {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct BeachMeta {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub settings: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct LayoutUpsert {
    pub preset: Option<String>,
    #[serde(default)]
    pub tiles: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct BeachLayout {
    pub preset: String,
    pub tiles: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ViewerCredentialResponse {
    pub credential_type: &'static str,
    pub credential: String,
    pub session_id: String,
    pub private_beach_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passcode: Option<String>,
}

pub async fn create_private_beach(
    State(state): State<AppState>,
    token: AuthToken,
    Json(body): Json<CreateBeachRequest>,
) -> ApiResult<BeachSummary> {
    ensure_scope(&token, "pb:beaches.write")?;
    let owner = token.account_uuid();
    let created = state
        .create_private_beach(&body.name, body.slug.as_deref(), owner)
        .await
        .map_err(map_state_err)?;
    Ok(Json(created))
}

pub async fn list_private_beaches(
    State(state): State<AppState>,
    token: AuthToken,
) -> ApiResult<Vec<BeachSummary>> {
    ensure_scope(&token, "pb:beaches.read")?;
    let list = state
        .list_private_beaches(token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(list))
}

pub async fn get_private_beach(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
) -> ApiResult<BeachMeta> {
    ensure_scope(&token, "pb:beaches.read")?;
    let meta = state
        .get_private_beach(&id, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(meta))
}

pub async fn update_private_beach(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
    Json(body): Json<UpdateBeachRequest>,
) -> ApiResult<BeachMeta> {
    ensure_scope(&token, "pb:beaches.write")?;
    let updated = state
        .update_private_beach(
            &id,
            body.name.as_deref(),
            body.slug.as_deref(),
            body.settings.clone(),
            token.account_uuid(),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(updated))
}

pub async fn get_private_beach_layout(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
) -> ApiResult<BeachLayout> {
    ensure_scope(&token, "pb:beaches.read")?;
    let layout = state
        .get_private_beach_layout(&id, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(layout))
}

pub async fn put_private_beach_layout(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
    Json(body): Json<LayoutUpsert>,
) -> ApiResult<serde_json::Value> {
    ensure_scope(&token, "pb:beaches.write")?;
    let preset = body.preset.unwrap_or_else(|| "grid2x2".to_string());
    let tiles = body.tiles.unwrap_or_default();
    state
        .put_private_beach_layout(&id, preset, tiles, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_viewer_credential(
    State(state): State<AppState>,
    token: AuthToken,
    Path((private_beach_id, session_id)): Path<(String, String)>,
) -> ApiResult<ViewerCredentialResponse> {
    ensure_scope(&token, "pb:sessions.read")?;
    let passcode = state
        .viewer_passcode(&private_beach_id, &session_id)
        .await
        .map_err(map_state_err)?
        .ok_or(ApiError::NotFound("viewer credential not available"))?;
    let issued_at = Some(Utc::now().timestamp_millis());
    match state
        .viewer_token(&session_id, &private_beach_id, &passcode)
        .await
    {
        Ok(issued) => Ok(Json(ViewerCredentialResponse {
            credential_type: "viewer_token",
            credential: issued.token,
            session_id,
            private_beach_id,
            issued_at_ms: issued_at,
            expires_at_ms: issued.expires_at_ms,
            passcode: Some(passcode),
        })),
        Err(ViewerTokenError::Unavailable | ViewerTokenError::Unauthorized) => {
            Ok(Json(ViewerCredentialResponse {
                credential_type: "viewer_passcode",
                credential: passcode.clone(),
                session_id,
                private_beach_id,
                issued_at_ms: issued_at,
                expires_at_ms: None,
                passcode: None,
            }))
        }
        Err(ViewerTokenError::Http(http_err)) => {
            warn!(error = %http_err, "viewer token http error");
            Err(ApiError::Upstream("viewer credential service failure"))
        }
        Err(ViewerTokenError::Upstream(msg)) => {
            warn!(message = %msg, "viewer token upstream error");
            Err(ApiError::Upstream("viewer credential service failure"))
        }
    }
}

fn map_state_err(err: StateError) -> ApiError {
    match err {
        StateError::SessionNotFound => ApiError::NotFound("session not found"),
        StateError::ControllerMismatch => ApiError::Conflict("controller mismatch"),
        StateError::PrivateBeachNotFound => ApiError::NotFound("private beach not found"),
        StateError::InvalidIdentifier(msg) => ApiError::BadRequest(msg),
        StateError::Database(e) => {
            warn!(error = %e, "database error");
            ApiError::Conflict("database error")
        }
        StateError::Redis(e) => {
            warn!(error = %e, "redis error");
            ApiError::Conflict("redis error")
        }
        StateError::Serde(e) => {
            warn!(error = %e, "serialization failure");
            ApiError::BadRequest("serialization error".into())
        }
        StateError::External(msg) => {
            warn!(message = %msg, "external dependency failure");
            ApiError::Upstream("external service failure")
        }
    }
}

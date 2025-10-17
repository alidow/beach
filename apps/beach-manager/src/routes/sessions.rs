use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    Json,
};
use beach_buggy::{
    ActionAck, ActionCommand, HealthHeartbeat, RegisterSessionRequest, RegisterSessionResponse,
    StateDiff,
};
use serde::Deserialize;
use tracing::error;

use crate::state::{
    AgentOnboardResponse, AppState, ControllerEvent, ControllerLeaseResponse, SessionSummary,
    StateError,
};

use super::{ApiError, ApiResult, AuthToken};

#[derive(Debug, Deserialize)]
pub struct SessionUpdateRequest {
    pub metadata: Option<serde_json::Value>,
    pub location_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ControllerLeaseRequest {
    pub requesting_account_id: Option<String>,
    pub ttl_ms: Option<u64>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseControllerRequest {
    pub controller_token: String,
}

#[derive(Debug, Deserialize)]
pub struct QueueActionsRequest {
    pub controller_token: String,
    pub actions: Vec<ActionCommand>,
}

#[derive(Debug, Deserialize)]
pub struct OnboardAgentRequest {
    pub session_id: String,
    pub template_id: String,
    #[serde(default)]
    pub scoped_roles: Vec<String>,
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,
}

pub async fn register_session(
    State(state): State<AppState>,
    token: AuthToken,
    Json(request): Json<RegisterSessionRequest>,
) -> ApiResult<RegisterSessionResponse> {
    let _ = token.as_str();
    let response = state
        .register_session(request)
        .await
        .map_err(map_state_err)?;
    Ok(Json(response))
}

pub async fn update_session(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<SessionUpdateRequest>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    state
        .update_session_metadata(&session_id, body.metadata, body.location_hint)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "updated": true })))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(private_beach_id): Path<String>,
) -> ApiResult<Vec<SessionSummary>> {
    let _ = token.as_str();
    let sessions = state
        .list_sessions(&private_beach_id)
        .await
        .map_err(map_state_err)?;
    Ok(Json(sessions))
}

pub async fn acquire_controller(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<ControllerLeaseRequest>,
) -> ApiResult<ControllerLeaseResponse> {
    let requester = body.requesting_account_id.clone();
    let _ = token.as_str();
    let response = state
        .acquire_controller(&session_id, body.ttl_ms, body.reason, requester)
        .await
        .map_err(map_state_err)?;
    Ok(Json(response))
}

pub async fn release_controller(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<ReleaseControllerRequest>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    state
        .release_controller(&session_id, &body.controller_token)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "released": true })))
}

pub async fn queue_actions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<QueueActionsRequest>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    if body.actions.is_empty() {
        return Err(ApiError::BadRequest("actions array required".into()));
    }

    state
        .queue_actions(&session_id, &body.controller_token, body.actions)
        .await
        .map_err(map_state_err)?;

    Ok(Json(serde_json::json!({ "accepted": true })))
}

pub async fn poll_actions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> ApiResult<Vec<ActionCommand>> {
    let _ = token.as_str();
    let commands = state
        .poll_actions(&session_id)
        .await
        .map_err(map_state_err)?;
    Ok(Json(commands))
}

pub async fn ack_actions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<Vec<ActionAck>>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    state
        .ack_actions(&session_id, body)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "acknowledged": true })))
}

pub async fn signal_health(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<HealthHeartbeat>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    state
        .record_health(&session_id, body)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "recorded": true })))
}

pub async fn push_state(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<StateDiff>,
) -> ApiResult<serde_json::Value> {
    let _ = token.as_str();
    state
        .record_state(&session_id, body)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "stored": true })))
}

pub async fn list_controller_events(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> ApiResult<Vec<ControllerEvent>> {
    let _ = token.as_str();
    let events = state
        .controller_events(&session_id)
        .await
        .map_err(map_state_err)?;
    Ok(Json(events))
}

pub async fn onboard_agent(
    State(state): State<AppState>,
    token: AuthToken,
    Json(body): Json<OnboardAgentRequest>,
) -> ApiResult<AgentOnboardResponse> {
    let _ = token.as_str();
    let response = state
        .onboard_agent(
            &body.session_id,
            &body.template_id,
            body.scoped_roles.clone(),
            body.options.clone(),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(response))
}

fn map_state_err(err: StateError) -> ApiError {
    match err {
        StateError::SessionNotFound => ApiError::NotFound("session not found"),
        StateError::ControllerMismatch => ApiError::Conflict("controller mismatch"),
        StateError::InvalidIdentifier(msg) => ApiError::BadRequest(msg),
        StateError::Database(e) => {
            error!(error = %e, "database operation failed");
            ApiError::Conflict("database error")
        }
        StateError::Redis(e) => {
            error!(error = %e, "redis operation failed");
            ApiError::Conflict("redis error")
        }
        StateError::Serde(e) => {
            error!(error = %e, "serialization failure");
            ApiError::BadRequest("serialization error".into())
        }
    }
}

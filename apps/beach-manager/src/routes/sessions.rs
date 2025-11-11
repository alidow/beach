use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use beach_buggy::{
    ActionAck, ActionCommand, HealthHeartbeat, RegisterSessionRequest, RegisterSessionResponse,
    StateDiff,
};
use reqwest::StatusCode;
use serde::Deserialize;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    log_throttle::{should_log_queue_event, QueueLogKind},
    state::{
        AgentOnboardResponse, AppState, ControllerEvent, ControllerLeaseResponse,
        ControllerPairing, ControllerUpdateCadence, JoinSessionResponsePayload, SessionSummary,
        StateError,
    },
};

use super::{ApiError, ApiResult, AuthToken};

pub(crate) fn ensure_scope(token: &AuthToken, scope: &'static str) -> Result<(), ApiError> {
    if token.has_scope(scope) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(scope))
    }
}

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

#[derive(Debug, Deserialize)]
pub struct EmergencyStopRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AttachByCodeRequest {
    pub session_id: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct AttachOwnedRequest {
    pub origin_session_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateControllerPairingRequest {
    pub child_session_id: String,
    pub prompt_template: Option<String>,
    pub update_cadence: Option<ControllerUpdateCadence>,
}

#[derive(Debug, Deserialize)]
pub struct JoinSessionRequestBody {
    pub passphrase: Option<String>,
    #[serde(default)]
    pub mcp: bool,
    #[serde(default)]
    pub viewer_token: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct AttachByCodeResponse {
    pub ok: bool,
    pub attach_method: &'static str,
    pub session: SessionSummary,
}

#[derive(Debug, serde::Serialize)]
pub struct AttachOwnedResponse {
    pub attached: usize,
    pub duplicates: usize,
}

pub async fn register_session(
    State(state): State<AppState>,
    token: AuthToken,
    Json(request): Json<RegisterSessionRequest>,
) -> ApiResult<RegisterSessionResponse> {
    ensure_scope(&token, "pb:sessions.register")?;
    info!(
        session_id = %request.session_id,
        private_beach_id = %request.private_beach_id,
        harness_type = ?request.harness_type,
        "register_session invoked"
    );
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
    ensure_scope(&token, "pb:sessions.write")?;
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
    headers: HeaderMap,
) -> ApiResult<Vec<SessionSummary>> {
    ensure_scope(&token, "pb:sessions.read")?;
    let sessions = state
        .list_sessions(&private_beach_id)
        .await
        .map_err(map_state_err)?;
    if let Some(trace_id) = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        info!(
            target: "trace.sessions",
            trace_id,
            private_beach_id = %private_beach_id,
            session_count = sessions.len(),
            "list_sessions trace request"
        );
    }
    Ok(Json(sessions))
}

pub async fn acquire_controller(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<ControllerLeaseRequest>,
) -> ApiResult<ControllerLeaseResponse> {
    ensure_scope(&token, "pb:control.write")?;
    let requester = token.account_uuid().or_else(|| {
        body.requesting_account_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok())
    });
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
    ensure_scope(&token, "pb:control.write")?;
    state
        .release_controller(&session_id, &body.controller_token, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "released": true })))
}

pub async fn list_controller_pairings_route(
    State(state): State<AppState>,
    token: AuthToken,
    Path(controller_session_id): Path<String>,
) -> ApiResult<Vec<ControllerPairing>> {
    if !(token.has_scope("pb:control.read") || token.has_scope("pb:sessions.read")) {
        return Err(ApiError::Forbidden("pb:control.read"));
    }
    let pairings = state
        .list_controller_pairings(&controller_session_id)
        .await
        .map_err(map_state_err)?;
    Ok(Json(pairings))
}

pub async fn create_controller_pairing(
    State(state): State<AppState>,
    token: AuthToken,
    Path(controller_session_id): Path<String>,
    Json(body): Json<CreateControllerPairingRequest>,
) -> ApiResult<ControllerPairing> {
    ensure_scope(&token, "pb:control.write")?;
    let pairing = state
        .upsert_controller_pairing(
            &controller_session_id,
            &body.child_session_id,
            body.prompt_template.clone(),
            body.update_cadence,
            token.account_uuid(),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(pairing))
}

pub async fn delete_controller_pairing(
    State(state): State<AppState>,
    token: AuthToken,
    Path((controller_session_id, child_session_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<serde_json::Value> {
    ensure_scope(&token, "pb:control.write")?;
    if let Some(trace_header) = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
    {
        info!(
            target: "controller.assignments",
            trace_id = trace_header,
            controller_session_id,
            child_session_id,
            "delete controller pairing request"
        );
    }
    state
        .delete_controller_pairing(
            &controller_session_id,
            &child_session_id,
            token.account_uuid(),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

pub async fn queue_actions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<QueueActionsRequest>,
) -> ApiResult<serde_json::Value> {
    ensure_scope(&token, "pb:control.write")?;
    if body.actions.is_empty() {
        return Err(ApiError::BadRequest("actions array required".into()));
    }
    if let Some(trace_id) = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
    {
        if should_log_queue_event(QueueLogKind::Request, &session_id) {
            info!(
                target: "controller.actions",
                trace_id,
                session_id,
                action_count = body.actions.len(),
                "queue_actions request"
            );
        }
    }

    state
        .queue_actions(
            &session_id,
            &body.controller_token,
            body.actions,
            token.account_uuid(),
        )
        .await
        .map_err(map_state_err)?;

    Ok(Json(serde_json::json!({ "accepted": true })))
}

pub async fn poll_actions(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> ApiResult<Vec<ActionCommand>> {
    ensure_scope(&token, "pb:control.consume")?;
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
    ensure_scope(&token, "pb:control.consume")?;
    state
        .ack_actions(&session_id, body, token.account_uuid(), false)
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
    ensure_scope(&token, "pb:harness.publish")?;
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
    ensure_scope(&token, "pb:harness.publish")?;
    let token_preview = token.as_str().chars().take(8).collect::<String>();
    info!(
        session_id = %session_id,
        sequence = body.sequence,
        token_preview = token_preview,
        "push_state received"
    );
    state
        .record_state(&session_id, body, false)
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "stored": true })))
}

pub async fn fetch_state_snapshot(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> ApiResult<Option<StateDiff>> {
    ensure_scope(&token, "pb:sessions.read")?;
    let snapshot = state
        .state_snapshot(&session_id)
        .await
        .map_err(map_state_err)?;
    Ok(Json(snapshot))
}

pub async fn list_controller_events(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Query(filter): Query<EventsFilter>,
) -> ApiResult<Vec<ControllerEvent>> {
    ensure_scope(&token, "pb:sessions.read")?;
    let events = state
        .controller_events_filtered(
            &session_id,
            filter.event_type.clone(),
            filter.since_ms,
            filter.limit.unwrap_or(200),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(events))
}

#[derive(Debug, Default, Deserialize)]
pub struct EventsFilter {
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub since_ms: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn onboard_agent(
    State(state): State<AppState>,
    token: AuthToken,
    headers: HeaderMap,
    Json(body): Json<OnboardAgentRequest>,
) -> ApiResult<AgentOnboardResponse> {
    ensure_scope(&token, "pb:agents.onboard")?;
    let response = state
        .onboard_agent(
            &body.session_id,
            &body.template_id,
            body.scoped_roles.clone(),
            body.options.clone(),
        )
        .await
        .map_err(map_state_err)?;
    if let Some(trace_id) = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        info!(
            target: "trace.agents",
            trace_id,
            session_id = %body.session_id,
            template_id = %body.template_id,
            bridge_count = response.mcp_bridges.len(),
            "agent onboard response tagged"
        );
    }
    Ok(Json(response))
}

pub async fn emergency_stop(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<EmergencyStopRequest>,
) -> ApiResult<serde_json::Value> {
    ensure_scope(&token, "pb:control.write")?;
    state
        .emergency_stop(&session_id, token.account_uuid(), body.reason.clone())
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "stopped": true })))
}

pub async fn attach_by_code(
    State(state): State<AppState>,
    token: AuthToken,
    Path(private_beach_id): Path<String>,
    Json(body): Json<AttachByCodeRequest>,
) -> ApiResult<AttachByCodeResponse> {
    ensure_scope(&token, "pb:sessions.write")?;
    let session = match state
        .attach_by_code(
            &private_beach_id,
            &body.session_id,
            &body.code,
            token.account_uuid(),
        )
        .await
    {
        Ok(session) => session,
        Err(err) => {
            warn!(
                target = "private_beach.sessions",
                private_beach_id = %private_beach_id,
                session_id = %body.session_id,
                error = %err,
                "session attach failed"
            );
            return Err(map_state_err(err));
        }
    };
    Ok(Json(AttachByCodeResponse {
        ok: true,
        attach_method: "code",
        session,
    }))
}

pub async fn attach_owned(
    State(state): State<AppState>,
    token: AuthToken,
    Path(private_beach_id): Path<String>,
    Json(body): Json<AttachOwnedRequest>,
) -> ApiResult<AttachOwnedResponse> {
    ensure_scope(&token, "pb:sessions.write")?;
    if body.origin_session_ids.is_empty() {
        return Err(ApiError::BadRequest("origin_session_ids required".into()));
    }
    let (attached, duplicates) = state
        .attach_owned(
            &private_beach_id,
            body.origin_session_ids.clone(),
            token.account_uuid(),
        )
        .await
        .map_err(map_state_err)?;
    Ok(Json(AttachOwnedResponse {
        attached,
        duplicates,
    }))
}

pub async fn join_session(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    Json(body): Json<JoinSessionRequestBody>,
) -> ApiResult<JoinSessionResponsePayload> {
    ensure_scope(&token, "pb:sessions.read")?;
    info!(
        target = "private_beach",
        session_id = %session_id,
        passphrase_provided = body
            .passphrase
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        viewer_token_provided = body
            .viewer_token
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        mcp = body.mcp,
        "join_session proxy request"
    );
    let (status, payload) = state
        .join_session_via_road(
            &session_id,
            body.passphrase.clone(),
            body.viewer_token.clone(),
            body.mcp,
        )
        .await
        .map_err(|err| {
            warn!(
                target = "private_beach",
                session_id = %session_id,
                error = %err,
                "failed to forward join request"
            );
            ApiError::Conflict("join proxy failed")
        })?;

    if status == StatusCode::NOT_FOUND {
        return Err(ApiError::NotFound("session not found"));
    }

    if !status.is_success() {
        let message = payload
            .message
            .clone()
            .unwrap_or_else(|| format!("join failed with status {}", status));
        return Err(ApiError::BadRequest(message));
    }

    info!(
        target = "private_beach",
        session_id = %session_id,
        status = %status,
        transports = ?payload.transports,
        "join_session proxy success"
    );
    Ok(Json(payload))
}

fn map_state_err(err: StateError) -> ApiError {
    match err {
        StateError::SessionNotFound => ApiError::NotFound("session not found"),
        StateError::ControllerMismatch => ApiError::Conflict("controller mismatch"),
        StateError::ControllerLeaseRequired => ApiError::Forbidden("controller lease required"),
        StateError::ControllerPairingNotFound => ApiError::NotFound("controller pairing not found"),
        StateError::CrossBeachPairing => {
            ApiError::BadRequest("sessions must belong to the same private beach".into())
        }
        StateError::PrivateBeachNotFound => ApiError::NotFound("private beach not found"),
        StateError::InvalidIdentifier(msg) => ApiError::BadRequest(msg),
        StateError::InvalidLayout(msg) => ApiError::BadRequest(msg),
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
        StateError::External(msg) => {
            error!(message = %msg, "external dependency failure");
            ApiError::Upstream("external service failure")
        }
        StateError::ActionQueueFull { .. } => {
            ApiError::TooManyRequests("pending controller action queue full")
        }
    }
}

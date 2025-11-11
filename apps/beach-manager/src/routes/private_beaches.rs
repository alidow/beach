use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasViewport {
    pub zoom: f64,
    pub pan: CanvasPoint,
}

impl Default for CanvasViewport {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: CanvasPoint::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasTileNode {
    pub id: String,
    pub position: CanvasPoint,
    pub size: CanvasSize,
    pub z_index: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolbar_pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasAgentNode {
    pub id: String,
    pub position: CanvasPoint,
    pub size: CanvasSize,
    pub z_index: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasGroupNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub member_ids: Vec<String>,
    pub position: CanvasPoint,
    pub size: CanvasSize,
    pub z_index: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasAssignment {
    pub controller_id: String,
    pub target_type: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CanvasAgentUpdateMode {
    #[serde(rename = "idle-summary")]
    IdleSummary,
    #[serde(rename = "push")]
    Push,
    #[serde(rename = "poll")]
    Poll,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CanvasAgentRelationship {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_handle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_handle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_mode: Option<CanvasAgentUpdateMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_frequency: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasMetadata {
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub agent_relationships: HashMap<String, CanvasAgentRelationship>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_relationship_order: Vec<String>,
}

impl Default for CanvasMetadata {
    fn default() -> Self {
        Self {
            created_at: 0,
            updated_at: 0,
            migrated_from: None,
            agent_relationships: HashMap::new(),
            agent_relationship_order: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasLayout {
    #[serde(default = "CanvasLayout::default_version")]
    pub version: u8,
    #[serde(default)]
    pub viewport: CanvasViewport,
    #[serde(default)]
    pub tiles: HashMap<String, CanvasTileNode>,
    #[serde(default)]
    pub agents: HashMap<String, CanvasAgentNode>,
    #[serde(default)]
    pub groups: HashMap<String, CanvasGroupNode>,
    #[serde(default)]
    pub control_assignments: HashMap<String, CanvasAssignment>,
    #[serde(default)]
    pub metadata: CanvasMetadata,
}

impl CanvasLayout {
    const fn default_version() -> u8 {
        3
    }

    pub fn empty(now_ms: i64) -> Self {
        Self {
            version: 3,
            viewport: CanvasViewport::default(),
            tiles: HashMap::new(),
            agents: HashMap::new(),
            groups: HashMap::new(),
            control_assignments: HashMap::new(),
            metadata: CanvasMetadata {
                created_at: now_ms,
                updated_at: now_ms,
                migrated_from: None,
                agent_relationships: HashMap::new(),
                agent_relationship_order: Vec::new(),
            },
        }
    }

    pub fn ensure_version(self) -> Result<Self, String> {
        if self.version != 3 {
            return Err("layout version must be 3".into());
        }
        Ok(self)
    }

    pub fn with_updated_timestamp(mut self, now_ms: i64) -> Self {
        if self.metadata.created_at == 0 {
            self.metadata.created_at = now_ms;
        }
        self.metadata.updated_at = now_ms;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canvas_layout_round_trips_tile_metadata() {
        let mut layout = CanvasLayout::empty(0);
        layout.tiles.insert(
            "agent-1".into(),
            CanvasTileNode {
                id: "agent-1".into(),
                position: CanvasPoint { x: 16.0, y: 24.0 },
                size: CanvasSize {
                    width: 320.0,
                    height: 240.0,
                },
                z_index: 4,
                group_id: None,
                zoom: Some(0.8),
                locked: Some(true),
                toolbar_pinned: Some(false),
                metadata: Some(json!({
                    "nodeType": "agent",
                    "agentMeta": {
                        "role": "Planner",
                        "responsibility": "Coordinate deploys",
                    },
                })),
            },
        );

        let serialized = serde_json::to_value(&layout).expect("serialize layout");
        let tile = serialized
            .get("tiles")
            .and_then(|tiles| tiles.get("agent-1"))
            .expect("tile entry");
        assert_eq!(
            tile.get("metadata")
                .and_then(|meta| meta.get("nodeType"))
                .and_then(|node| node.as_str()),
            Some("agent"),
            "metadata should be serialized alongside tile"
        );

        let round_tripped: CanvasLayout =
            serde_json::from_value(serialized).expect("deserialize layout");
        let round_tile = round_tripped.tiles.get("agent-1").expect("tile entry");
        assert_eq!(
            round_tile
                .metadata
                .as_ref()
                .and_then(|meta| meta.get("agentMeta"))
                .and_then(|meta| meta.get("role"))
                .and_then(|role| role.as_str()),
            Some("Planner"),
            "metadata should round-trip through serde"
        );
    }
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

#[derive(Debug, Deserialize)]
pub struct BatchAssignmentItem {
    pub controller_session_id: String,
    pub child_session_id: String,
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub update_cadence: Option<crate::state::ControllerUpdateCadence>,
}

#[derive(Debug, Serialize)]
pub struct BatchAssignmentResultItem {
    pub controller_session_id: String,
    pub child_session_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing: Option<crate::state::ControllerPairing>,
}

#[derive(Debug, Deserialize)]
pub struct BatchAssignmentsRequest {
    pub assignments: Vec<BatchAssignmentItem>,
}

#[derive(Debug, Serialize)]
pub struct BatchAssignmentsResponse {
    pub results: Vec<BatchAssignmentResultItem>,
}

pub async fn batch_controller_assignments(
    State(state): State<AppState>,
    token: AuthToken,
    Path(_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BatchAssignmentsRequest>,
) -> ApiResult<BatchAssignmentsResponse> {
    ensure_scope(&token, "pb:control.write")?;
    if body.assignments.is_empty() {
        return Err(ApiError::BadRequest("assignments array required".into()));
    }
    if let Some(trace_header) = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
    {
        info!(
            target: "controller.assignments",
            trace_id = trace_header,
            assignments = body.assignments.len(),
            "batch controller assignments request"
        );
    }
    let mut results = Vec::with_capacity(body.assignments.len());
    for item in body.assignments.into_iter() {
        let res = match state
            .upsert_controller_pairing(
                &item.controller_session_id,
                &item.child_session_id,
                item.prompt_template.clone(),
                item.update_cadence,
                token.account_uuid(),
            )
            .await
        {
            Ok(pairing) => BatchAssignmentResultItem {
                controller_session_id: item.controller_session_id,
                child_session_id: item.child_session_id,
                ok: true,
                error: None,
                pairing: Some(pairing),
            },
            Err(e) => BatchAssignmentResultItem {
                controller_session_id: item.controller_session_id,
                child_session_id: item.child_session_id,
                ok: false,
                error: Some(format!("{}", e)),
                pairing: None,
            },
        };
        results.push(res);
    }
    Ok(Json(BatchAssignmentsResponse { results }))
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
) -> ApiResult<CanvasLayout> {
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
    Json(body): Json<CanvasLayout>,
) -> ApiResult<CanvasLayout> {
    ensure_scope(&token, "pb:beaches.write")?;
    let layout = state
        .put_private_beach_layout(&id, body, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(layout))
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
        StateError::ControllerLeaseRequired => ApiError::Forbidden("controller lease required"),
        StateError::ControllerPairingNotFound => ApiError::NotFound("controller pairing not found"),
        StateError::CrossBeachPairing => {
            ApiError::BadRequest("sessions must belong to the same private beach".into())
        }
        StateError::PrivateBeachNotFound => ApiError::NotFound("private beach not found"),
        StateError::InvalidIdentifier(msg) => ApiError::BadRequest(msg),
        StateError::InvalidLayout(msg) => ApiError::BadRequest(msg),
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
        StateError::ActionQueueFull { .. } => {
            ApiError::TooManyRequests("pending controller action queue full")
        }
    }
}

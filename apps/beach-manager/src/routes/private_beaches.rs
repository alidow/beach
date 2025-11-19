use axum::{
    async_trait,
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::state::{
    AppState, AttachHandshakeDisposition, ControllerCommandDropReason, ControllerPairing,
    ControllerUpdateCadence, SessionSummary, StateError, ViewerTokenError,
};

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphRequest {
    pub tiles: Vec<SessionGraphTile>,
    #[serde(default)]
    pub relationships: Vec<SessionGraphRelationship>,
    #[serde(default)]
    pub viewport: Option<CanvasViewport>,
    #[serde(default)]
    pub clear_existing: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphTile {
    pub id: String,
    #[serde(default)]
    pub node_type: SessionGraphNodeType,
    pub position: CanvasPoint,
    pub size: CanvasSize,
    #[serde(default)]
    pub z_index: Option<i32>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub zoom: Option<f64>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub toolbar_pinned: Option<bool>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub session: Option<SessionGraphTileSession>,
    #[serde(default)]
    pub agent: Option<SessionGraphAgentSpec>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionGraphNodeType {
    Application,
    Agent,
}

impl Default for SessionGraphNodeType {
    fn default() -> Self {
        SessionGraphNodeType::Application
    }
}

impl SessionGraphNodeType {
    fn as_str(&self) -> &'static str {
        match self {
            SessionGraphNodeType::Application => "application",
            SessionGraphNodeType::Agent => "agent",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphTileSession {
    pub session_id: String,
    pub code: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphAgentSpec {
    pub role: String,
    #[serde(default)]
    pub responsibility: Option<String>,
    #[serde(default)]
    pub trace: Option<SessionGraphAgentTrace>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphAgentTrace {
    pub enabled: bool,
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphRelationship {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    #[serde(default)]
    pub source_handle_id: Option<String>,
    #[serde(default)]
    pub target_handle_id: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub update_mode: Option<CanvasAgentUpdateMode>,
    #[serde(default)]
    pub poll_frequency: Option<i64>,
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub update_cadence: Option<ControllerUpdateCadence>,
}

#[derive(Debug, Serialize)]
pub struct SessionGraphAttachmentResult {
    pub tile_id: String,
    pub session_id: String,
    pub method: &'static str,
    pub handshake_dispatched: bool,
}

#[derive(Debug, Serialize)]
pub struct SessionGraphPairingResult {
    pub relationship_id: String,
    pub controller_session_id: String,
    pub child_session_id: String,
    pub pairing: ControllerPairing,
}

#[derive(Debug, Serialize)]
pub struct SessionGraphResponse {
    pub layout: CanvasLayout,
    pub attachments: Vec<SessionGraphAttachmentResult>,
    pub pairings: Vec<SessionGraphPairingResult>,
}

#[derive(Debug, Deserialize)]
pub struct ShowcasePreflightQuery {
    #[serde(default)]
    pub refresh: Option<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShowcasePreflightIssue {
    pub code: String,
    pub severity: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShowcasePreflightResponse {
    pub status: String,
    pub issues: Vec<ShowcasePreflightIssue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,
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

pub async fn install_session_graph(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
    Json(body): Json<SessionGraphRequest>,
) -> ApiResult<SessionGraphResponse> {
    ensure_scope(&token, "pb:beaches.write")?;
    ensure_scope(&token, "pb:sessions.write")?;
    ensure_scope(&token, "pb:control.write")?;
    if body.tiles.is_empty() {
        return Err(ApiError::BadRequest("tiles array required".into()));
    }
    let SessionGraphRequest {
        tiles,
        relationships,
        viewport,
        clear_existing,
    } = body;
    let account = token.account_uuid();
    let mut request_tile_ids = HashSet::new();
    for tile in &tiles {
        if tile.id.trim().is_empty() {
            return Err(ApiError::BadRequest("tile id is required".into()));
        }
        if !request_tile_ids.insert(tile.id.clone()) {
            return Err(ApiError::BadRequest(format!(
                "duplicate tile id {}",
                tile.id
            )));
        }
        if matches!(tile.node_type, SessionGraphNodeType::Agent) && tile.agent.is_none() {
            return Err(ApiError::BadRequest(format!(
                "agent tile {} requires agent metadata",
                tile.id
            )));
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    let mut layout = if clear_existing {
        CanvasLayout::empty(now_ms)
    } else {
        state
            .get_private_beach_layout(&id, account)
            .await
            .map_err(map_state_err)?
    };
    let mut session_lookup: HashMap<String, String> = if clear_existing {
        HashMap::new()
    } else {
        layout
            .tiles
            .iter()
            .filter_map(|(tile_id, node)| {
                extract_session_id_from_tile(node).map(|session_id| (tile_id.clone(), session_id))
            })
            .collect()
    };
    let mut known_tiles: HashSet<String> = if clear_existing {
        HashSet::new()
    } else {
        layout.tiles.keys().cloned().collect()
    };
    known_tiles.extend(request_tile_ids.iter().cloned());

    let mut attachments = Vec::new();
    let mut attached_summaries: HashMap<String, SessionSummary> = HashMap::new();
    for tile in &tiles {
        if let Some(session_spec) = &tile.session {
            let session_id = session_spec.session_id.trim();
            if session_id.is_empty() {
                return Err(ApiError::BadRequest(format!(
                    "tile {} session_id is required",
                    tile.id
                )));
            }
            let code = session_spec.code.trim();
            if code.is_empty() {
                return Err(ApiError::BadRequest(format!(
                    "tile {} code is required",
                    tile.id
                )));
            }
            let outcome = state
                .attach_by_code(
                    &id,
                    session_id,
                    code,
                    account,
                    AttachHandshakeDisposition::Dispatch,
                )
                .await
                .map_err(map_state_err)?;
            session_lookup.insert(tile.id.clone(), outcome.session.session_id.clone());
            attached_summaries.insert(tile.id.clone(), outcome.session.clone());
            attachments.push(SessionGraphAttachmentResult {
                tile_id: tile.id.clone(),
                session_id: outcome.session.session_id.clone(),
                method: "code",
                handshake_dispatched: outcome.handshake_dispatched,
            });
        }
    }

    let mut max_z = layout
        .tiles
        .values()
        .map(|node| node.z_index)
        .max()
        .unwrap_or(0);
    for tile in &tiles {
        let base = layout.tiles.get(&tile.id);
        let mut metadata_map = metadata_map_from(
            base.and_then(|node| node.metadata.as_ref()),
            tile.metadata.clone(),
        )?;
        metadata_map.insert(
            "nodeType".into(),
            serde_json::Value::String(tile.node_type.as_str().into()),
        );
        if let Some(summary) = attached_summaries.get(&tile.id) {
            let meta = session_meta_from_summary(summary, tile.session.as_ref());
            metadata_map.insert("sessionMeta".into(), meta);
        }
        match tile.node_type {
            SessionGraphNodeType::Agent => {
                if let Some(agent_spec) = tile.agent.as_ref() {
                    metadata_map.insert("agentMeta".into(), agent_meta_from_spec(agent_spec));
                }
            }
            SessionGraphNodeType::Application => {
                metadata_map.remove("agentMeta");
            }
        }
        let metadata_value = if metadata_map.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(metadata_map))
        };
        let z_index = tile.z_index.unwrap_or_else(|| {
            max_z += 1;
            max_z
        });
        layout.tiles.insert(
            tile.id.clone(),
            CanvasTileNode {
                id: tile.id.clone(),
                position: tile.position.clone(),
                size: tile.size.clone(),
                z_index,
                group_id: tile
                    .group_id
                    .clone()
                    .or_else(|| base.and_then(|node| node.group_id.clone())),
                zoom: tile.zoom.or_else(|| base.and_then(|node| node.zoom)),
                locked: tile.locked.or_else(|| base.and_then(|node| node.locked)),
                toolbar_pinned: tile
                    .toolbar_pinned
                    .or_else(|| base.and_then(|node| node.toolbar_pinned)),
                metadata: metadata_value,
            },
        );
    }

    if let Some(viewport) = viewport {
        layout.viewport = viewport;
    }

    let mut relationships_map = if clear_existing {
        HashMap::new()
    } else {
        layout.metadata.agent_relationships.clone()
    };
    let mut relationship_order = if clear_existing {
        Vec::new()
    } else {
        layout.metadata.agent_relationship_order.clone()
    };
    let mut seen_relationships = HashSet::new();
    let mut pairings = Vec::new();

    for relationship in &relationships {
        if relationship.id.trim().is_empty() {
            return Err(ApiError::BadRequest("relationship id is required".into()));
        }
        if !seen_relationships.insert(relationship.id.clone()) {
            return Err(ApiError::BadRequest(format!(
                "duplicate relationship id {}",
                relationship.id
            )));
        }
        if !known_tiles.contains(&relationship.source_id) {
            return Err(ApiError::BadRequest(format!(
                "relationship {} references unknown source tile {}",
                relationship.id, relationship.source_id
            )));
        }
        if !known_tiles.contains(&relationship.target_id) {
            return Err(ApiError::BadRequest(format!(
                "relationship {} references unknown target tile {}",
                relationship.id, relationship.target_id
            )));
        }
        let controller_session_id = session_lookup
            .get(&relationship.source_id)
            .cloned()
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "relationship {} source {} does not have an attached session",
                    relationship.id, relationship.source_id
                ))
            })?;
        let child_session_id = session_lookup
            .get(&relationship.target_id)
            .cloned()
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "relationship {} target {} does not have an attached session",
                    relationship.id, relationship.target_id
                ))
            })?;
        let poll_frequency = sanitize_poll_frequency(relationship.poll_frequency)?;
        relationships_map.insert(
            relationship.id.clone(),
            CanvasAgentRelationship {
                id: relationship.id.clone(),
                source_id: relationship.source_id.clone(),
                target_id: relationship.target_id.clone(),
                source_handle_id: relationship.source_handle_id.clone(),
                target_handle_id: relationship.target_handle_id.clone(),
                instructions: relationship.instructions.clone(),
                update_mode: relationship.update_mode.clone(),
                poll_frequency,
            },
        );
        relationship_order.retain(|entry| entry != &relationship.id);
        relationship_order.push(relationship.id.clone());

        let cadence = relationship
            .update_cadence
            .or_else(|| cadence_from_update_mode(&relationship.update_mode));
        let pairing = state
            .upsert_controller_pairing(
                &controller_session_id,
                &child_session_id,
                relationship.prompt_template.clone(),
                cadence,
                account,
            )
            .await
            .map_err(map_state_err)?;
        pairings.push(SessionGraphPairingResult {
            relationship_id: relationship.id.clone(),
            controller_session_id,
            child_session_id,
            pairing,
        });
    }

    layout.metadata.agent_relationships = relationships_map;
    layout.metadata.agent_relationship_order = relationship_order;

    let layout = state
        .put_private_beach_layout(&id, layout, account)
        .await
        .map_err(map_state_err)?;

    Ok(Json(SessionGraphResponse {
        layout,
        attachments,
        pairings,
    }))
}

const DEFAULT_SHOWCASE_ACCOUNT: &str = "00000000-0000-0000-0000-000000000001";

struct ShowcaseTileRequirement {
    role: &'static str,
    tile_id: &'static str,
}

const SHOWCASE_TILE_REQUIREMENTS: &[ShowcaseTileRequirement] = &[
    ShowcaseTileRequirement {
        role: "agent",
        tile_id: "pong-agent",
    },
    ShowcaseTileRequirement {
        role: "lhs",
        tile_id: "pong-lhs",
    },
    ShowcaseTileRequirement {
        role: "rhs",
        tile_id: "pong-rhs",
    },
];

#[derive(Debug, Clone)]
struct ShowcaseTileSession {
    session_id: String,
}

struct ShowcasePreflightContext<'a> {
    state: &'a AppState,
    private_beach_id: Uuid,
    private_beach_id_str: String,
    account: Option<Uuid>,
    layout: Option<Arc<CanvasLayout>>,
    resolved_tiles: HashMap<&'static str, ShowcaseTileSession>,
}

impl<'a> ShowcasePreflightContext<'a> {
    fn new(state: &'a AppState, private_beach_id: Uuid, account: Option<Uuid>) -> Self {
        Self {
            state,
            private_beach_id,
            private_beach_id_str: private_beach_id.to_string(),
            account,
            layout: None,
            resolved_tiles: HashMap::new(),
        }
    }

    fn state(&self) -> &AppState {
        self.state
    }

    fn private_beach_id(&self) -> &Uuid {
        &self.private_beach_id
    }

    async fn layout(&mut self) -> Result<Arc<CanvasLayout>, StateError> {
        if self.layout.is_none() {
            let layout = self
                .state
                .get_private_beach_layout(&self.private_beach_id_str, self.account)
                .await?;
            self.layout = Some(Arc::new(layout));
        }
        Ok(self.layout.as_ref().expect("layout populated").clone())
    }

    fn set_resolved_tiles(&mut self, map: HashMap<&'static str, ShowcaseTileSession>) {
        self.resolved_tiles = map;
    }

    fn tile_for_role(&self, role: &'static str) -> Option<&ShowcaseTileSession> {
        self.resolved_tiles.get(role)
    }
}

#[async_trait]
trait ShowcasePreflightCheck {
    async fn run(
        &self,
        ctx: &mut ShowcasePreflightContext<'_>,
    ) -> Result<Vec<ShowcasePreflightIssue>, StateError>;
}

struct RequiredAccountCheck {
    accounts: Vec<Uuid>,
}

#[async_trait]
impl ShowcasePreflightCheck for RequiredAccountCheck {
    async fn run(
        &self,
        ctx: &mut ShowcasePreflightContext<'_>,
    ) -> Result<Vec<ShowcasePreflightIssue>, StateError> {
        let mut issues = Vec::new();
        for account_id in &self.accounts {
            if !ctx.state().account_active(*account_id).await? {
                issues.push(ShowcasePreflightIssue {
                    code: "missing_account".into(),
                    severity: "error".into(),
                    detail: format!("required host account {} is not active", account_id),
                    remediation: Some(
                        "run scripts/db-seed to create the host user and retry".into(),
                    ),
                });
            }
        }
        Ok(issues)
    }
}

struct TileAttachmentCheck {
    requirements: &'static [ShowcaseTileRequirement],
}

#[async_trait]
impl ShowcasePreflightCheck for TileAttachmentCheck {
    async fn run(
        &self,
        ctx: &mut ShowcasePreflightContext<'_>,
    ) -> Result<Vec<ShowcasePreflightIssue>, StateError> {
        let mut issues = Vec::new();
        let layout = ctx.layout().await?;
        let mut resolved = HashMap::new();
        for req in self.requirements {
            match layout.tiles.get(req.tile_id) {
                Some(tile) => {
                    if let Some(session_id) = session_id_from_tile(tile) {
                        match Uuid::parse_str(&session_id) {
                            Ok(session_uuid) => {
                                if ctx
                                    .state()
                                    .session_attached_to_beach(
                                        &session_uuid,
                                        ctx.private_beach_id(),
                                    )
                                    .await?
                                {
                                    let resolved_id = session_id.clone();
                                    resolved.insert(
                                        req.role,
                                        ShowcaseTileSession {
                                            session_id: resolved_id,
                                        },
                                    );
                                } else {
                                    issues.push(ShowcasePreflightIssue {
                                        code: "session_missing".into(),
                                        severity: "error".into(),
                                        detail: format!(
                                            "session {} from tile '{}' is not attached to this beach",
                                            session_id, req.tile_id
                                        ),
                                        remediation: Some(
                                            "attach the Tile via the dashboard or rerun pong-stack".
                                                into(),
                                        ),
                                    });
                                }
                            }
                            Err(_) => {
                                issues.push(ShowcasePreflightIssue {
                                    code: "session_invalid".into(),
                                    severity: "error".into(),
                                    detail: format!(
                                        "tile '{}' contains invalid session identifier",
                                        req.tile_id
                                    ),
                                    remediation: Some(
                                        "ensure the tile metadata contains a valid sessionId"
                                            .into(),
                                    ),
                                });
                            }
                        }
                    } else {
                        issues.push(ShowcasePreflightIssue {
                            code: "tile_missing_session".into(),
                            severity: "error".into(),
                            detail: format!("tile '{}' lacks session metadata", req.tile_id),
                            remediation: Some(
                                "attach a session to the tile before starting the showcase".into(),
                            ),
                        });
                    }
                }
                None => {
                    issues.push(ShowcasePreflightIssue {
                        code: "tile_missing".into(),
                        severity: "error".into(),
                        detail: format!("required tile '{}' is not present", req.tile_id),
                        remediation: Some(
                            "drop the tile from the catalog and persist the layout".into(),
                        ),
                    });
                }
            }
        }
        ctx.set_resolved_tiles(resolved);
        Ok(issues)
    }
}

struct PairingHealthCheck {
    requirements: &'static [ShowcaseTileRequirement],
}

#[async_trait]
impl ShowcasePreflightCheck for PairingHealthCheck {
    async fn run(
        &self,
        ctx: &mut ShowcasePreflightContext<'_>,
    ) -> Result<Vec<ShowcasePreflightIssue>, StateError> {
        let mut issues = Vec::new();
        let Some(agent) = ctx.tile_for_role("agent") else {
            issues.push(ShowcasePreflightIssue {
                code: "agent_missing".into(),
                severity: "error".into(),
                detail: "agent session is unavailable; ensure the agent tile is attached".into(),
                remediation: Some("re-attach the agent tile or restart the showcase stack".into()),
            });
            return Ok(issues);
        };
        let pairings = ctx
            .state()
            .list_controller_pairings(&agent.session_id)
            .await?
            .into_iter()
            .map(|pair| pair.child_session_id)
            .collect::<HashSet<_>>();
        let mut missing = Vec::new();
        for req in self.requirements {
            if req.role == "agent" {
                continue;
            }
            if let Some(child) = ctx.tile_for_role(req.role) {
                if !pairings.contains(&child.session_id) {
                    missing.push(child.session_id.clone());
                }
            }
        }
        if !missing.is_empty() {
            issues.push(ShowcasePreflightIssue {
                code: "pairing_missing".into(),
                severity: "error".into(),
                detail: format!(
                    "agent session is not paired with child sessions: {}",
                    missing.join(", ")
                ),
                remediation: Some(
                    "create the Agent→Player connections from the canvas and re-save".into(),
                ),
            });
        }
        Ok(issues)
    }
}

struct FastPathWarningCheck;

#[async_trait]
impl ShowcasePreflightCheck for FastPathWarningCheck {
    async fn run(
        &self,
        ctx: &mut ShowcasePreflightContext<'_>,
    ) -> Result<Vec<ShowcasePreflightIssue>, StateError> {
        let mut issues = Vec::new();
        if let Some(agent) = ctx.tile_for_role("agent") {
            if !ctx.state().is_fast_path_ready(&agent.session_id).await {
                issues.push(ShowcasePreflightIssue {
                    code: "fast_path_pending".into(),
                    severity: "warning".into(),
                    detail: format!(
                        "controller session {} has not connected via fast-path yet",
                        agent.session_id
                    ),
                    remediation: Some(
                        "wait for the agent harness to complete fast-path upgrade or restart it"
                            .into(),
                    ),
                });
            }
        }
        Ok(issues)
    }
}

pub async fn showcase_preflight(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
    Query(query): Query<ShowcasePreflightQuery>,
) -> ApiResult<ShowcasePreflightResponse> {
    ensure_scope(&token, "pb:beaches.read")?;
    let beach_uuid = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return Err(ApiError::BadRequest("invalid private beach id".into()));
        }
    };
    let refresh = query.refresh.unwrap_or_default() > 0;
    if let Some(mut cached) = state.cached_showcase_preflight(&id, refresh).await {
        cached.cached = Some(true);
        return Ok(Json(cached));
    }

    let mut ctx = ShowcasePreflightContext::new(&state, beach_uuid, token.account_uuid());
    let mut checks: Vec<Box<dyn ShowcasePreflightCheck + Send + Sync>> = Vec::new();
    checks.push(Box::new(RequiredAccountCheck {
        accounts: parse_showcase_required_accounts(),
    }));
    checks.push(Box::new(TileAttachmentCheck {
        requirements: SHOWCASE_TILE_REQUIREMENTS,
    }));
    checks.push(Box::new(PairingHealthCheck {
        requirements: SHOWCASE_TILE_REQUIREMENTS,
    }));
    checks.push(Box::new(FastPathWarningCheck));

    let mut issues = Vec::new();
    for check in checks {
        let mut result = check.run(&mut ctx).await.map_err(map_state_err)?;
        issues.append(&mut result);
    }

    let status = if issues.iter().any(|issue| issue.severity == "error") {
        "blocked".to_string()
    } else {
        "ok".to_string()
    };
    if status == "blocked" {
        warn!(
            target = "controller.leases",
            private_beach_id = %id,
            issue_count = issues.len(),
            "showcase preflight blocked"
        );
    }
    let response = ShowcasePreflightResponse {
        status,
        issues,
        cached: None,
    };
    let mut cacheable = response.clone();
    cacheable.cached = None;
    state.store_showcase_preflight(&id, cacheable).await;
    Ok(Json(response))
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

pub async fn delete_private_beach(
    State(state): State<AppState>,
    token: AuthToken,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    ensure_scope(&token, "pb:beaches.write")?;
    state
        .delete_private_beach(&id, token.account_uuid())
        .await
        .map_err(map_state_err)?;
    Ok(Json(serde_json::json!({ "deleted": true })))
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
        StateError::AccountMissing(account) => ApiError::ConflictWithCode {
            message: format!(
                "controller account {} is not registered in this cluster",
                account
            ),
            code: "account_missing",
        },
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
        StateError::Internal(msg) => {
            error!(message = %msg, "internal controller error");
            ApiError::Internal
        }
        StateError::ActionQueueFull { .. } => {
            ApiError::TooManyRequests("pending controller action queue full")
        }
        StateError::ControllerCommandRejected { reason } => match reason {
            ControllerCommandDropReason::FastPathNotReady => ApiError::PreconditionFailed {
                message: reason.default_message().to_string(),
                code: reason.code(),
            },
            _ => ApiError::ConflictWithCode {
                message: reason.default_message().to_string(),
                code: reason.code(),
            },
        },
    }
}

fn metadata_map_from(
    existing: Option<&serde_json::Value>,
    overrides: Option<serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    let mut base = existing
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_else(serde_json::Map::new);
    if let Some(payload) = overrides {
        match payload {
            serde_json::Value::Null => base.clear(),
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    base.insert(key, value);
                }
            }
            _ => {
                return Err(ApiError::BadRequest(
                    "tile metadata must be an object".into(),
                ))
            }
        }
    }
    Ok(base)
}

fn session_meta_from_summary(
    summary: &SessionSummary,
    overrides: Option<&SessionGraphTileSession>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let override_title = overrides
        .and_then(|spec| spec.title.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let derived_title = summary
        .metadata
        .as_ref()
        .and_then(|meta| meta.as_object())
        .and_then(|obj| {
            obj.get("title")
                .or_else(|| obj.get("name"))
                .and_then(|value| value.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let title = override_title
        .or(derived_title)
        .unwrap_or_else(|| summary.session_id.clone());
    map.insert("sessionId".into(), serde_json::json!(summary.session_id));
    map.insert("title".into(), serde_json::json!(title));
    map.insert("status".into(), serde_json::json!("attached"));
    if let Some(harness) = serde_json::to_value(&summary.harness_type)
        .ok()
        .and_then(|value| value.as_str().map(|s| s.to_string()))
    {
        map.insert("harnessType".into(), serde_json::json!(harness));
    }
    map.insert(
        "pendingActions".into(),
        serde_json::json!(summary.pending_actions as i64),
    );
    serde_json::Value::Object(map)
}

fn agent_meta_from_spec(spec: &SessionGraphAgentSpec) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("role".into(), serde_json::json!(spec.role.clone()));
    map.insert(
        "responsibility".into(),
        serde_json::json!(spec.responsibility.clone().unwrap_or_default()),
    );
    map.insert("isEditing".into(), serde_json::json!(false));
    if let Some(trace) = spec.trace.as_ref() {
        let mut trace_map = serde_json::Map::new();
        trace_map.insert("enabled".into(), serde_json::json!(trace.enabled));
        if let Some(id) = trace
            .trace_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            trace_map.insert("trace_id".into(), serde_json::json!(id));
        }
        map.insert("trace".into(), serde_json::Value::Object(trace_map));
    }
    serde_json::Value::Object(map)
}

fn extract_session_id_from_tile(node: &CanvasTileNode) -> Option<String> {
    let metadata = node.metadata.as_ref()?.as_object()?;
    let session_meta = metadata.get("sessionMeta")?.as_object()?;
    session_meta
        .get("sessionId")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn cadence_from_update_mode(
    mode: &Option<CanvasAgentUpdateMode>,
) -> Option<ControllerUpdateCadence> {
    match mode {
        Some(CanvasAgentUpdateMode::Push) => Some(ControllerUpdateCadence::Fast),
        Some(CanvasAgentUpdateMode::Poll) => Some(ControllerUpdateCadence::Slow),
        Some(CanvasAgentUpdateMode::IdleSummary) => Some(ControllerUpdateCadence::Slow),
        None => None,
    }
}

fn sanitize_poll_frequency(value: Option<i64>) -> Result<Option<i64>, ApiError> {
    const MIN_POLL_SECS: i64 = 1;
    const MAX_POLL_SECS: i64 = 86_400;
    if let Some(freq) = value {
        if freq < MIN_POLL_SECS || freq > MAX_POLL_SECS {
            return Err(ApiError::BadRequest(
                "pollFrequency must be between 1 and 86400 seconds".into(),
            ));
        }
        Ok(Some(freq))
    } else {
        Ok(None)
    }
}

fn parse_showcase_required_accounts() -> Vec<Uuid> {
    let mut ids = Vec::new();
    if let Ok(raw) = env::var("PONG_SHOWCASE_REQUIRED_ACCOUNTS") {
        for token in raw.split(',').map(|value| value.trim()) {
            if token.is_empty() {
                continue;
            }
            if let Ok(uuid) = Uuid::parse_str(token) {
                ids.push(uuid);
            }
        }
    }
    if ids.is_empty() {
        if let Ok(uuid) = Uuid::parse_str(DEFAULT_SHOWCASE_ACCOUNT) {
            ids.push(uuid);
        }
    }
    ids
}

fn session_id_from_tile(node: &CanvasTileNode) -> Option<String> {
    let metadata = node.metadata.as_ref()?.as_object()?;
    let session_meta = metadata.get("sessionMeta")?.as_object()?;
    session_meta
        .get("sessionId")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

#[cfg(test)]
mod showcase_preflight_tests {
    use super::*;
    use crate::routes::build_router;
    use axum::{body::Body, http::Request};
    use beach_buggy::{HarnessType, RegisterSessionRequest, TransportMode};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    fn session_tile(id: &str, session_id: &str) -> CanvasTileNode {
        CanvasTileNode {
            id: id.into(),
            position: CanvasPoint { x: 0.0, y: 0.0 },
            size: CanvasSize {
                width: 100.0,
                height: 80.0,
            },
            z_index: 0,
            group_id: None,
            zoom: None,
            locked: None,
            toolbar_pinned: None,
            metadata: Some(json!({
                "sessionMeta": {
                    "sessionId": session_id,
                    "title": id,
                    "status": "attached"
                }
            })),
        }
    }

    async fn build_showcase_state(state: &AppState, beach_id: &str) -> (String, String, String) {
        let agent = Uuid::new_v4().to_string();
        let lhs = Uuid::new_v4().to_string();
        let rhs = Uuid::new_v4().to_string();
        for session in [&agent, &lhs, &rhs] {
            let req = RegisterSessionRequest {
                session_id: session.clone(),
                private_beach_id: beach_id.to_string(),
                harness_type: HarnessType::TerminalShim,
                capabilities: vec![],
                location_hint: None,
                metadata: None,
                version: "test".into(),
                viewer_passcode: None,
                transport_mode: Some(TransportMode::FastPath),
            };
            state.register_session(req).await.unwrap();
        }
        state
            .acquire_controller(&agent, Some(30_000), Some("test".into()), None)
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let mut layout = CanvasLayout::empty(now);
        layout
            .tiles
            .insert("pong-agent".into(), session_tile("pong-agent", &agent));
        layout
            .tiles
            .insert("pong-lhs".into(), session_tile("pong-lhs", &lhs));
        layout
            .tiles
            .insert("pong-rhs".into(), session_tile("pong-rhs", &rhs));
        state
            .put_private_beach_layout(beach_id, layout, None)
            .await
            .unwrap();
        (agent, lhs, rhs)
    }

    #[tokio::test]
    async fn showcase_preflight_reports_ok_with_valid_setup() {
        let state = AppState::new();
        let beach_id = Uuid::new_v4().to_string();
        let (agent, lhs, rhs) = build_showcase_state(&state, &beach_id).await;
        state
            .upsert_controller_pairing(&agent, &lhs, None, None, None)
            .await
            .unwrap();
        state
            .upsert_controller_pairing(&agent, &rhs, None, None, None)
            .await
            .unwrap();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/private-beaches/{}/showcase-preflight", beach_id))
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let payload: ShowcasePreflightResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload.status, "ok");
        assert!(payload.issues.iter().all(|issue| issue.severity != "error"));
    }

    #[tokio::test]
    async fn showcase_preflight_blocks_when_pairings_missing() {
        let state = AppState::new();
        let beach_id = Uuid::new_v4().to_string();
        let (_agent, _lhs, _rhs) = build_showcase_state(&state, &beach_id).await;
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/private-beaches/{}/showcase-preflight", beach_id))
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let payload: ShowcasePreflightResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload.status, "blocked");
        assert!(payload
            .issues
            .iter()
            .any(|issue| issue.code == "pairing_missing" && issue.severity == "error"));
    }
}

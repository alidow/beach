use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use beach_buggy::{
    ActionAck, ActionCommand, HarnessType, HealthHeartbeat, RegisterSessionRequest,
    RegisterSessionResponse, StateDiff,
};
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgPool, Row};
use tokio::sync::RwLock;
use uuid::Uuid;

const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
const REDIS_ACTION_STREAM_MAXLEN: usize = 2_048;
const REDIS_TTL_SECONDS: usize = 120;

#[derive(Clone)]
pub struct AppState {
    backend: Backend,
    fallback: Arc<InnerState>,
    redis: Option<Arc<redis::Client>>,
}

#[derive(Clone)]
enum Backend {
    Memory,
    Postgres(PgPool),
}

struct InnerState {
    sessions: RwLock<HashMap<String, SessionRecord>>,
}

#[derive(Debug, Clone)]
struct SessionRecord {
    session_id: String,
    private_beach_id: String,
    harness_type: HarnessType,
    capabilities: Vec<String>,
    location_hint: Option<String>,
    metadata: Option<serde_json::Value>,
    version: String,
    harness_id: String,
    controller_token: Option<String>,
    lease_ttl_ms: u64,
    transport_hints: HashMap<String, serde_json::Value>,
    state_cache_url: Option<String>,
    pending_actions: VecDeque<ActionCommand>,
    controller_events: Vec<ControllerEvent>,
    last_health: Option<HealthHeartbeat>,
    last_state: Option<StateDiff>,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("session not found")]
    SessionNotFound,
    #[error("controller token mismatch")]
    ControllerMismatch,
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub private_beach_id: String,
    pub harness_type: HarnessType,
    pub capabilities: Vec<String>,
    pub location_hint: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub version: String,
    pub harness_id: String,
    pub controller_token: Option<String>,
    pub pending_actions: usize,
    pub last_health: Option<HealthHeartbeat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControllerEvent {
    pub id: String,
    pub event_type: ControllerEventType,
    pub controller_token: Option<String>,
    pub timestamp_ms: i64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerEventType {
    Registered,
    LeaseAcquired,
    LeaseReleased,
    ActionsQueued,
    ActionsAcked,
    HealthReported,
    StateUpdated,
}

#[derive(Debug, Clone, Serialize)]
pub struct ControllerLeaseResponse {
    pub controller_token: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentOnboardResponse {
    pub agent_token: String,
    pub prompt_pack: serde_json::Value,
    pub mcp_bridges: Vec<McpBridge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpBridge {
    pub id: String,
    pub name: String,
    pub description: String,
    pub endpoint: Option<String>,
}

#[derive(Debug)]
struct DbSessionIdentifiers {
    session_id: Uuid,
    private_beach_id: Uuid,
    harness_type: Option<String>,
    harness_id: Option<Uuid>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            backend: Backend::Memory,
            fallback: Arc::new(InnerState::new()),
            redis: None,
        }
    }

    pub fn with_db(pool: PgPool) -> Self {
        Self {
            backend: Backend::Postgres(pool),
            fallback: Arc::new(InnerState::new()),
            redis: None,
        }
    }

    pub fn with_redis(mut self, client: redis::Client) -> Self {
        self.redis = Some(Arc::new(client));
        self
    }

    #[allow(dead_code)]
    pub fn db_pool(&self) -> Option<&PgPool> {
        match &self.backend {
            Backend::Postgres(pool) => Some(pool),
            Backend::Memory => None,
        }
    }

    pub async fn register_session(
        &self,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        match &self.backend {
            Backend::Memory => self.register_session_memory(req).await,
            Backend::Postgres(pool) => self.register_session_postgres(pool, req).await,
        }
    }

    pub async fn update_session_metadata(
        &self,
        session_id: &str,
        metadata: Option<serde_json::Value>,
        location_hint: Option<String>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                let mut sessions = self.fallback.sessions.write().await;
                let record = sessions
                    .get_mut(session_id)
                    .ok_or(StateError::SessionNotFound)?;
                if let Some(meta) = metadata {
                    record.metadata = Some(meta);
                }
                if let Some(loc) = location_hint {
                    record.location_hint = Some(loc);
                }
                Ok(())
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let (meta_json, loc) = (
                    metadata.unwrap_or_else(|| serde_json::json!({})),
                    location_hint,
                );
                let result = sqlx::query(
                    r#"
                    UPDATE session
                    SET metadata = $2,
                        location_hint = COALESCE($3, location_hint),
                        last_seen_at = NOW()
                    WHERE origin_session_id = $1
                    "#,
                )
                .bind(session_uuid)
                .bind(Json(meta_json))
                .bind(loc)
                .execute(pool)
                .await?;

                if result.rows_affected() == 0 {
                    return Err(StateError::SessionNotFound);
                }
                Ok(())
            }
        }
    }

    pub async fn list_sessions(
        &self,
        private_beach_id: &str,
    ) -> Result<Vec<SessionSummary>, StateError> {
        match &self.backend {
            Backend::Memory => {
                let sessions = self.fallback.sessions.read().await;
                let summaries = sessions
                    .values()
                    .filter(|record| record.private_beach_id == private_beach_id)
                    .map(SessionSummary::from_record)
                    .collect();
                Ok(summaries)
            }
            Backend::Postgres(pool) => {
                let beach_uuid = parse_uuid(private_beach_id, "private_beach_id")?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        s.origin_session_id,
                        s.private_beach_id,
                        s.harness_id,
                        s.harness_type,
                        s.capabilities,
                        s.metadata,
                        s.location_hint,
                        sr.last_health,
                        cl.id AS controller_token,
                        cl.expires_at,
                        cl.revoked_at
                    FROM session s
                    LEFT JOIN session_runtime sr ON sr.session_id = s.id
                    LEFT JOIN controller_lease cl ON cl.session_id = s.id
                    WHERE s.private_beach_id = $1
                    ORDER BY s.created_at ASC
                    "#,
                )
                .bind(beach_uuid)
                .fetch_all(pool)
                .await?;

                let mut summaries = Vec::with_capacity(rows.len());
                for row in rows {
                    let origin_session: Uuid = row.try_get("origin_session_id")?;
                    let harness_id: Option<Uuid> = row.try_get("harness_id")?;
                    let harness_type = row.try_get::<Option<String>, _>("harness_type");
                    let harness = harness_type
                        .as_deref()
                        .and_then(harness_type_from_db)
                        .unwrap_or(HarnessType::Custom);
                    let capabilities: Json<serde_json::Value> = row.try_get("capabilities")?;
                    let metadata: Json<serde_json::Value> = row.try_get("metadata")?;
                    let location_hint: Option<String> = row.try_get("location_hint")?;
                    let controller_token: Option<Uuid> = row.try_get("controller_token")?;
                    let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at")?;
                    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
                    let last_health: Option<Json<serde_json::Value>> =
                        row.try_get("last_health").ok();

                    let pending_actions = self
                        .pending_actions_count(&beach_uuid.to_string(), &origin_session.to_string())
                        .await?;
                    let controller_token = controller_token
                        .filter(|_| is_active_lease(expires_at, revoked_at))
                        .map(|token| token.to_string());
                    let last_health =
                        last_health.and_then(|Json(value)| serde_json::from_value(value).ok());

                    summaries.push(SessionSummary {
                        session_id: origin_session.to_string(),
                        private_beach_id: beach_uuid.to_string(),
                        harness_type: harness,
                        capabilities: json_array_to_strings(&capabilities.0),
                        location_hint,
                        metadata: Some(metadata.0),
                        version: "unknown".into(),
                        harness_id: harness_id.unwrap_or_else(Uuid::new_v4).to_string(),
                        controller_token,
                        pending_actions,
                        last_health,
                    });
                }
                Ok(summaries)
            }
        }
    }

    pub async fn acquire_controller(
        &self,
        session_id: &str,
        ttl_override: Option<u64>,
        reason: Option<String>,
        requester: Option<String>,
    ) -> Result<ControllerLeaseResponse, StateError> {
        match &self.backend {
            Backend::Memory => self.acquire_controller_memory(session_id, ttl_override, reason),
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let ttl = ttl_override.unwrap_or(DEFAULT_LEASE_TTL_MS).max(1_000);
                let expires_at = Utc::now() + Duration::milliseconds(ttl as i64);
                let controller_token = Uuid::new_v4();
                let requester_uuid = requester.as_deref().and_then(|s| Uuid::parse_str(s).ok());

                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO controller_lease (
                        id, session_id, controller_account_id, issued_by_account_id,
                        reason, issued_at, expires_at, revoked_at
                    )
                    VALUES ($1, $2, $3, $3, $4, NOW(), $5, NULL)
                    ON CONFLICT (session_id)
                    DO UPDATE SET
                        id = EXCLUDED.id,
                        controller_account_id = EXCLUDED.controller_account_id,
                        issued_by_account_id = EXCLUDED.issued_by_account_id,
                        reason = EXCLUDED.reason,
                        issued_at = NOW(),
                        expires_at = EXCLUDED.expires_at,
                        revoked_at = NULL
                    "#,
                )
                .bind(controller_token)
                .bind(identifiers.session_id)
                .bind(requester_uuid)
                .bind(reason.clone())
                .bind(expires_at)
                .execute(&mut *tx)
                .await?;

                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_acquired",
                    Some(controller_token),
                    requester_uuid,
                    reason,
                )
                .await?;

                tx.commit().await?;

                self.fallback
                    .acknowledge_controller(session_id, controller_token.to_string(), ttl)
                    .await;

                Ok(ControllerLeaseResponse {
                    controller_token: controller_token.to_string(),
                    expires_at_ms: expires_at.timestamp_millis(),
                })
            }
        }
    }

    pub async fn release_controller(
        &self,
        session_id: &str,
        controller_token: &str,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => {
                self.release_controller_memory(session_id, controller_token)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let token_uuid = parse_uuid(controller_token, "controller_token")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                let mut tx = pool.begin().await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE controller_lease
                    SET revoked_at = NOW(), expires_at = NOW()
                    WHERE session_id = $1 AND id = $2
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(token_uuid)
                .execute(&mut *tx)
                .await?;

                if updated.rows_affected() == 0 {
                    return Err(StateError::ControllerMismatch);
                }

                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "lease_released",
                    Some(token_uuid),
                    None,
                    None,
                )
                .await?;

                tx.commit().await?;
                self.fallback.clear_controller(session_id).await;
                Ok(())
            }
        }
    }

    pub async fn queue_actions(
        &self,
        session_id: &str,
        controller_token: &str,
        actions: Vec<ActionCommand>,
    ) -> Result<(), StateError> {
        if actions.is_empty() {
            return Ok(());
        }

        match &self.backend {
            Backend::Memory => {
                self.queue_actions_memory(session_id, controller_token, actions)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let token_uuid = parse_uuid(controller_token, "controller_token")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let lease = self
                    .fetch_active_lease(pool, identifiers.session_id)
                    .await?;

                if lease.id != token_uuid {
                    return Err(StateError::ControllerMismatch);
                }

                let now = Utc::now();
                if lease
                    .expires_at
                    .map(|expires_at| expires_at < now)
                    .unwrap_or(true)
                {
                    return Err(StateError::ControllerMismatch);
                }

                self.enqueue_actions_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    actions.clone(),
                )
                .await?;

                let mut tx = pool.begin().await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "actions_queued",
                    Some(lease.id),
                    lease.controller_account_id,
                    None,
                )
                .await?;
                tx.commit().await?;

                self.fallback.enqueue_actions(session_id, actions).await;

                Ok(())
            }
        }
    }

    pub async fn poll_actions(&self, session_id: &str) -> Result<Vec<ActionCommand>, StateError> {
        match &self.backend {
            Backend::Memory => self.poll_actions_memory(session_id).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let actions = self
                    .drain_actions_redis(
                        &identifiers.private_beach_id.to_string(),
                        &session_uuid.to_string(),
                    )
                    .await?;
                if actions.is_empty() {
                    return Ok(actions);
                }

                let mut tx = pool.begin().await?;
                if let Some(lease) = self
                    .fetch_active_lease_optional(&mut tx, identifiers.session_id)
                    .await?
                {
                    self.insert_controller_event(
                        &mut tx,
                        identifiers.session_id,
                        "actions_acked",
                        Some(lease.id),
                        lease.controller_account_id,
                        None,
                    )
                    .await?;
                }
                tx.commit().await?;
                self.fallback.clear_pending_actions(session_id).await;
                Ok(actions)
            }
        }
    }

    pub async fn ack_actions(
        &self,
        session_id: &str,
        _acks: Vec<ActionAck>,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => Ok(()),
            Backend::Postgres(_) => Ok(()),
        }
    }

    pub async fn record_health(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => self.record_health_memory(session_id, heartbeat).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                self.store_health_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    &heartbeat,
                )
                .await?;

                sqlx::query(
                    r#"
                    INSERT INTO session_runtime (session_id, last_health, last_health_at)
                    VALUES ($1, $2, NOW())
                    ON CONFLICT (session_id)
                    DO UPDATE SET last_health = EXCLUDED.last_health, last_health_at = NOW()
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(Json(serde_json::to_value(&heartbeat)?))
                .execute(pool)
                .await?;

                let mut tx = pool.begin().await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "health_reported",
                    None,
                    None,
                    None,
                )
                .await?;
                tx.commit().await?;

                self.fallback.store_health(session_id, heartbeat).await;
                Ok(())
            }
        }
    }

    pub async fn record_state(&self, session_id: &str, diff: StateDiff) -> Result<(), StateError> {
        match &self.backend {
            Backend::Memory => self.record_state_memory(session_id, diff).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                self.store_state_redis(
                    &identifiers.private_beach_id.to_string(),
                    &session_uuid.to_string(),
                    &diff,
                )
                .await?;

                sqlx::query(
                    r#"
                    INSERT INTO session_runtime (session_id, last_state, last_state_at)
                    VALUES ($1, $2, NOW())
                    ON CONFLICT (session_id)
                    DO UPDATE SET last_state = EXCLUDED.last_state, last_state_at = NOW()
                    "#,
                )
                .bind(identifiers.session_id)
                .bind(Json(serde_json::to_value(&diff)?))
                .execute(pool)
                .await?;

                let mut tx = pool.begin().await?;
                self.insert_controller_event(
                    &mut tx,
                    identifiers.session_id,
                    "state_updated",
                    None,
                    None,
                    None,
                )
                .await?;
                tx.commit().await?;

                self.fallback.store_state(session_id, diff).await;
                Ok(())
            }
        }
    }

    pub async fn controller_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<ControllerEvent>, StateError> {
        match &self.backend {
            Backend::Memory => self.controller_events_memory(session_id).await,
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;
                let rows = sqlx::query(
                    r#"
                    SELECT id, event_type, controller_token, reason, occurred_at
                    FROM controller_event
                    WHERE session_id = $1
                    ORDER BY occurred_at DESC
                    LIMIT 200
                    "#,
                )
                .bind(identifiers.session_id)
                .fetch_all(pool)
                .await?;

                let events = rows
                    .into_iter()
                    .filter_map(|row| {
                        let id: Uuid = row.try_get("id").ok()?;
                        let event_type: String = row.try_get("event_type").ok()?;
                        let controller_token: Option<Uuid> =
                            row.try_get("controller_token").ok()?;
                        let reason: Option<String> = row.try_get("reason").ok()?;
                        let occurred_at: DateTime<Utc> = row.try_get("occurred_at").ok()?;
                        Some(ControllerEvent {
                            id: id.to_string(),
                            event_type: controller_event_from_str(&event_type),
                            controller_token: controller_token.map(|uuid| uuid.to_string()),
                            timestamp_ms: occurred_at.timestamp_millis(),
                            reason,
                        })
                    })
                    .collect();
                Ok(events)
            }
        }
    }

    pub async fn onboard_agent(
        &self,
        session_id: &str,
        template_id: &str,
        scoped_roles: Vec<String>,
        options: HashMap<String, serde_json::Value>,
    ) -> Result<AgentOnboardResponse, StateError> {
        match &self.backend {
            Backend::Memory => {
                self.onboard_agent_memory(session_id, template_id, scoped_roles, options)
                    .await
            }
            Backend::Postgres(pool) => {
                let session_uuid = parse_uuid(session_id, "session_id")?;
                let identifiers = self.fetch_session_identifiers(pool, &session_uuid).await?;

                let prompt_pack = serde_json::json!({
                    "template_id": template_id,
                    "session_id": session_uuid,
                    "private_beach_id": identifiers.private_beach_id,
                    "instructions": "You are the designated automation manager for this Private Beach. Follow lease rules and only execute authorized actions.",
                    "scoped_roles": scoped_roles,
                    "options": options,
                });
                let response = AgentOnboardResponse {
                    agent_token: Uuid::new_v4().to_string(),
                    prompt_pack,
                    mcp_bridges: vec![
                        McpBridge {
                            id: "beach_state".into(),
                            name: "Beach State".into(),
                            description: "Read the latest terminal/GUI state for any session"
                                .into(),
                            endpoint: Some("private_beach.subscribe_state".into()),
                        },
                        McpBridge {
                            id: "beach_action".into(),
                            name: "Beach Action".into(),
                            description:
                                "Send actions (keystrokes, pointer events) to sessions you control"
                                    .into(),
                            endpoint: Some("private_beach.queue_action".into()),
                        },
                    ],
                };
                Ok(response)
            }
        }
    }
}

impl InnerState {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    async fn ensure_session(
        &self,
        req: &RegisterSessionRequest,
        harness_id: &str,
        controller_token: Option<String>,
    ) {
        let mut sessions = self.sessions.write().await;
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(&req.session_id, &req.private_beach_id, &req.harness_type)
        });
        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();
        entry.harness_id = harness_id.to_string();
        entry.controller_token = controller_token;
    }

    async fn acknowledge_controller(&self, session_id: &str, token: String, ttl: u64) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.controller_token = Some(token);
            record.lease_ttl_ms = ttl;
        }
    }

    async fn clear_controller(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.controller_token = None;
        }
    }

    async fn enqueue_actions(&self, session_id: &str, actions: Vec<ActionCommand>) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            for action in actions {
                record.pending_actions.push_back(action);
            }
        }
    }

    async fn clear_pending_actions(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.pending_actions.clear();
        }
    }

    async fn store_health(&self, session_id: &str, heartbeat: HealthHeartbeat) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.last_health = Some(heartbeat);
        }
    }

    async fn store_state(&self, session_id: &str, diff: StateDiff) {
        let mut sessions = self.sessions.write().await;
        if let Some(record) = sessions.get_mut(session_id) {
            record.last_state = Some(diff);
        }
    }
}

impl SessionRecord {
    fn new(session_id: &str, private_beach_id: &str, harness_type: &HarnessType) -> Self {
        let harness_id = Uuid::new_v4().to_string();
        let transport_hints = default_transport_hints(session_id);
        Self {
            session_id: session_id.to_string(),
            private_beach_id: private_beach_id.to_string(),
            harness_type: harness_type.clone(),
            capabilities: Vec::new(),
            location_hint: None,
            metadata: None,
            version: "unknown".into(),
            harness_id,
            controller_token: None,
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            transport_hints,
            state_cache_url: None,
            pending_actions: VecDeque::new(),
            controller_events: Vec::new(),
            last_health: None,
            last_state: None,
        }
    }

    fn append_event(&mut self, event_type: ControllerEventType, reason: Option<String>) {
        self.controller_events.push(ControllerEvent {
            id: Uuid::new_v4().to_string(),
            event_type,
            controller_token: self.controller_token.clone(),
            timestamp_ms: now_ms(),
            reason,
        });
    }
}

impl SessionSummary {
    fn from_record(record: &SessionRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            private_beach_id: record.private_beach_id.clone(),
            harness_type: record.harness_type.clone(),
            capabilities: record.capabilities.clone(),
            location_hint: record.location_hint.clone(),
            metadata: record.metadata.clone(),
            version: record.version.clone(),
            harness_id: record.harness_id.clone(),
            controller_token: record.controller_token.clone(),
            pending_actions: record.pending_actions.len(),
            last_health: record.last_health.clone(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_uuid(value: &str, label: &str) -> Result<Uuid, StateError> {
    Uuid::parse_str(value).map_err(|_| StateError::InvalidIdentifier(format!("{label}={value}")))
}

fn harness_type_to_db(value: &HarnessType) -> &'static str {
    match value {
        HarnessType::TerminalShim => "terminal_shim",
        HarnessType::CabanaAdapter => "cabana_adapter",
        HarnessType::RemoteWidget => "remote_widget",
        HarnessType::ServiceProxy => "service_proxy",
        HarnessType::Custom => "custom",
    }
}

fn harness_type_from_db(value: &str) -> Option<HarnessType> {
    match value {
        "terminal_shim" => Some(HarnessType::TerminalShim),
        "cabana_adapter" => Some(HarnessType::CabanaAdapter),
        "remote_widget" => Some(HarnessType::RemoteWidget),
        "service_proxy" => Some(HarnessType::ServiceProxy),
        "custom" => Some(HarnessType::Custom),
        _ => None,
    }
}

fn harness_to_session_kind(value: &HarnessType) -> &'static str {
    match value {
        HarnessType::TerminalShim => "terminal",
        HarnessType::CabanaAdapter => "cabana_gui",
        HarnessType::RemoteWidget => "widget",
        HarnessType::ServiceProxy => "service_daemon",
        HarnessType::Custom => "widget",
    }
}

fn default_transport_hints(session_id: &str) -> HashMap<String, serde_json::Value> {
    let mut hints = HashMap::new();
    hints.insert(
        "actions_poll".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/actions/poll") }),
    );
    hints.insert(
        "actions_ack".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/actions/ack") }),
    );
    hints.insert(
        "state_post".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/state") }),
    );
    hints.insert(
        "health_post".into(),
        serde_json::json!({ "path": format!("/sessions/{session_id}/health") }),
    );
    hints
}

fn controller_event_from_str(value: &str) -> ControllerEventType {
    match value {
        "lease_acquired" => ControllerEventType::LeaseAcquired,
        "lease_released" => ControllerEventType::LeaseReleased,
        "actions_queued" => ControllerEventType::ActionsQueued,
        "actions_acked" => ControllerEventType::ActionsAcked,
        "health_reported" => ControllerEventType::HealthReported,
        "state_updated" => ControllerEventType::StateUpdated,
        _ => ControllerEventType::Registered,
    }
}

fn json_array_to_strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn is_active_lease(expires_at: Option<DateTime<Utc>>, revoked_at: Option<DateTime<Utc>>) -> bool {
    let now = Utc::now();
    revoked_at.is_none() && expires_at.map(|exp| exp > now).unwrap_or(false)
}

fn redis_actions_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:actions")
}

fn redis_health_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:health")
}

fn redis_state_key(private_beach_id: &str, session_id: &str) -> String {
    format!("pb:{private_beach_id}:sess:{session_id}:state")
}

impl AppState {
    async fn register_session_memory(
        &self,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let entry = sessions.entry(req.session_id.clone()).or_insert_with(|| {
            SessionRecord::new(&req.session_id, &req.private_beach_id, &req.harness_type)
        });

        entry.capabilities = req.capabilities.clone();
        entry.location_hint = req.location_hint.clone();
        entry.metadata = req.metadata.clone();
        entry.version = req.version.clone();
        entry.harness_type = req.harness_type.clone();

        if entry.controller_token.is_none() {
            entry.controller_token = Some(Uuid::new_v4().to_string());
        }

        entry.append_event(ControllerEventType::Registered, None);

        Ok(RegisterSessionResponse {
            harness_id: entry.harness_id.clone(),
            controller_token: entry.controller_token.clone(),
            lease_ttl_ms: entry.lease_ttl_ms,
            state_cache_url: entry.state_cache_url.clone(),
            transport_hints: entry.transport_hints.clone(),
        })
    }

    async fn register_session_postgres(
        &self,
        pool: &PgPool,
        req: RegisterSessionRequest,
    ) -> Result<RegisterSessionResponse, StateError> {
        let session_uuid = parse_uuid(&req.session_id, "session_id")?;
        let private_beach_uuid = parse_uuid(&req.private_beach_id, "private_beach_id")?;
        let harness_id = Uuid::new_v4();
        let controller_token = Uuid::new_v4();
        let kind = harness_to_session_kind(&req.harness_type);
        let transport_hints = default_transport_hints(&req.session_id);
        let transport_json = serde_json::to_value(&transport_hints)?;
        let capabilities = serde_json::Value::Array(
            req.capabilities
                .iter()
                .map(|cap| serde_json::Value::String(cap.clone()))
                .collect(),
        );
        let metadata = req
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let expires_at = Utc::now() + Duration::milliseconds(DEFAULT_LEASE_TTL_MS as i64);

        let mut tx = pool.begin().await?;

        let session_row = sqlx::query(
            r#"
            INSERT INTO session (
                id, private_beach_id, origin_session_id, harness_id, kind,
                location_hint, capabilities, metadata, harness_type, last_seen_at
            )
            VALUES ($1, $2, $3, $4, $5::session_kind, $6, $7, $8, $9::harness_type, NOW())
            ON CONFLICT (private_beach_id, origin_session_id)
            DO UPDATE SET
                harness_id = EXCLUDED.harness_id,
                kind = EXCLUDED.kind,
                location_hint = EXCLUDED.location_hint,
                capabilities = EXCLUDED.capabilities,
                metadata = EXCLUDED.metadata,
                harness_type = EXCLUDED.harness_type,
                last_seen_at = NOW()
            RETURNING id
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(private_beach_uuid)
        .bind(session_uuid)
        .bind(harness_id)
        .bind(kind)
        .bind(req.location_hint.clone())
        .bind(Json(capabilities))
        .bind(Json(metadata))
        .bind(harness_type_to_db(&req.harness_type))
        .fetch_one(&mut *tx)
        .await?;
        let db_session_id: Uuid = session_row.try_get("id")?;

        sqlx::query(
            r#"
            INSERT INTO session_runtime (session_id, transport_hints)
            VALUES ($1, $2)
            ON CONFLICT (session_id)
            DO UPDATE SET transport_hints = EXCLUDED.transport_hints
            "#,
        )
        .bind(db_session_id)
        .bind(Json(transport_json))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO controller_lease (
                id, session_id, controller_account_id, issued_by_account_id,
                reason, issued_at, expires_at, revoked_at
            )
            VALUES ($1, $2, NULL, NULL, NULL, NOW(), $3, NULL)
            ON CONFLICT (session_id)
            DO UPDATE SET
                id = EXCLUDED.id,
                controller_account_id = NULL,
                issued_by_account_id = NULL,
                reason = NULL,
                issued_at = NOW(),
                expires_at = EXCLUDED.expires_at,
                revoked_at = NULL
            "#,
        )
        .bind(controller_token)
        .bind(db_session_id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;

        self.insert_controller_event(
            &mut tx,
            db_session_id,
            "registered",
            Some(controller_token),
            None,
            None,
        )
        .await?;

        self.insert_controller_event(
            &mut tx,
            db_session_id,
            "lease_acquired",
            Some(controller_token),
            None,
            None,
        )
        .await?;

        tx.commit().await?;

        self.fallback
            .ensure_session(
                &req,
                &harness_id.to_string(),
                Some(controller_token.to_string()),
            )
            .await;

        Ok(RegisterSessionResponse {
            harness_id: harness_id.to_string(),
            controller_token: Some(controller_token.to_string()),
            lease_ttl_ms: DEFAULT_LEASE_TTL_MS,
            state_cache_url: None,
            transport_hints,
        })
    }

    async fn acquire_controller_memory(
        &self,
        session_id: &str,
        ttl_override: Option<u64>,
        reason: Option<String>,
    ) -> Result<ControllerLeaseResponse, StateError> {
        let ttl = ttl_override.unwrap_or(DEFAULT_LEASE_TTL_MS);
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;

        record.lease_ttl_ms = ttl;
        record.controller_token = Some(Uuid::new_v4().to_string());
        record.append_event(ControllerEventType::LeaseAcquired, reason);

        Ok(ControllerLeaseResponse {
            controller_token: record.controller_token.clone().unwrap(),
            expires_at_ms: now_ms() + ttl as i64,
        })
    }

    async fn release_controller_memory(
        &self,
        session_id: &str,
        controller_token: &str,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        if record.controller_token.as_deref() != Some(controller_token) {
            return Err(StateError::ControllerMismatch);
        }
        record.controller_token = None;
        record.append_event(ControllerEventType::LeaseReleased, None);
        Ok(())
    }

    async fn queue_actions_memory(
        &self,
        session_id: &str,
        controller_token: &str,
        actions: Vec<ActionCommand>,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        if record.controller_token.as_deref() != Some(controller_token) {
            return Err(StateError::ControllerMismatch);
        }
        for action in actions {
            record.pending_actions.push_back(action);
        }
        record.append_event(ControllerEventType::ActionsQueued, None);
        Ok(())
    }

    async fn poll_actions_memory(
        &self,
        session_id: &str,
    ) -> Result<Vec<ActionCommand>, StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let actions = record.pending_actions.drain(..).collect();
        Ok(actions)
    }

    async fn record_health_memory(
        &self,
        session_id: &str,
        heartbeat: HealthHeartbeat,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        record.last_health = Some(heartbeat.clone());
        record.append_event(ControllerEventType::HealthReported, None);
        Ok(())
    }

    async fn record_state_memory(
        &self,
        session_id: &str,
        diff: StateDiff,
    ) -> Result<(), StateError> {
        let mut sessions = self.fallback.sessions.write().await;
        let record = sessions
            .get_mut(session_id)
            .ok_or(StateError::SessionNotFound)?;
        record.last_state = Some(diff);
        record.append_event(ControllerEventType::StateUpdated, None);
        Ok(())
    }

    async fn controller_events_memory(
        &self,
        session_id: &str,
    ) -> Result<Vec<ControllerEvent>, StateError> {
        let sessions = self.fallback.sessions.read().await;
        let record = sessions
            .get(session_id)
            .ok_or(StateError::SessionNotFound)?;
        Ok(record.controller_events.clone())
    }

    async fn onboard_agent_memory(
        &self,
        session_id: &str,
        template_id: &str,
        scoped_roles: Vec<String>,
        options: HashMap<String, serde_json::Value>,
    ) -> Result<AgentOnboardResponse, StateError> {
        let sessions = self.fallback.sessions.read().await;
        let record = sessions
            .get(session_id)
            .ok_or(StateError::SessionNotFound)?;
        let prompt_pack = serde_json::json!({
            "template_id": template_id,
            "session_id": record.session_id,
            "private_beach_id": record.private_beach_id,
            "instructions": "You are the designated automation manager for this Private Beach. Follow lease rules and only execute authorized actions.",
            "scoped_roles": scoped_roles,
            "options": options,
        });
        Ok(AgentOnboardResponse {
            agent_token: Uuid::new_v4().to_string(),
            prompt_pack,
            mcp_bridges: vec![
                McpBridge {
                    id: "beach_state".into(),
                    name: "Beach State".into(),
                    description: "Read the latest terminal/GUI state for any session".into(),
                    endpoint: Some("private_beach.subscribe_state".into()),
                },
                McpBridge {
                    id: "beach_action".into(),
                    name: "Beach Action".into(),
                    description:
                        "Send actions (keystrokes, pointer events) to sessions you control".into(),
                    endpoint: Some("private_beach.queue_action".into()),
                },
            ],
        })
    }

    async fn fetch_session_identifiers(
        &self,
        pool: &PgPool,
        session_uuid: &Uuid,
    ) -> Result<DbSessionIdentifiers, StateError> {
        let row = sqlx::query(
            r#"
            SELECT id, private_beach_id, harness_type, harness_id
            FROM session
            WHERE origin_session_id = $1
            "#,
        )
        .bind(session_uuid)
        .fetch_optional(pool)
        .await?;

        if let Some(row) = row {
            Ok(DbSessionIdentifiers {
                session_id: row.try_get("id")?,
                private_beach_id: row.try_get("private_beach_id")?,
                harness_type: row.try_get("harness_type").ok(),
                harness_id: row.try_get("harness_id").ok(),
            })
        } else {
            Err(StateError::SessionNotFound)
        }
    }

    async fn fetch_active_lease(
        &self,
        pool: &PgPool,
        session_id: Uuid,
    ) -> Result<LeaseRow, StateError> {
        let row = sqlx::query(
            r#"
            SELECT id, controller_account_id, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

        let lease = row
            .and_then(|record| {
                let revoked_at: Option<DateTime<Utc>> = record.try_get("revoked_at").ok()?;
                let expires_at: Option<DateTime<Utc>> = record.try_get("expires_at").ok()?;
                let id: Uuid = record.try_get("id").ok()?;
                Some(LeaseRow {
                    id,
                    controller_account_id: record.try_get("controller_account_id").ok()?,
                    expires_at,
                    revoked_at,
                })
            })
            .ok_or(StateError::ControllerMismatch)?;
        Ok(lease)
    }

    async fn fetch_active_lease_optional(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
    ) -> Result<Option<LeaseRow>, StateError> {
        let row = sqlx::query(
            r#"
            SELECT id, controller_account_id, expires_at, revoked_at
            FROM controller_lease
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;

        Ok(row.and_then(|record| {
            let id: Uuid = record.try_get("id").ok()?;
            let controller_account_id = record.try_get("controller_account_id").ok();
            let expires_at = record.try_get("expires_at").ok();
            let revoked_at = record.try_get("revoked_at").ok();
            Some(LeaseRow {
                id,
                controller_account_id,
                expires_at,
                revoked_at,
            })
        }))
    }

    async fn insert_controller_event(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
        event_type: &str,
        controller_token: Option<Uuid>,
        controller_account_id: Option<Uuid>,
        reason: Option<String>,
    ) -> Result<(), StateError> {
        sqlx::query(
            r#"
            INSERT INTO controller_event (
                id, session_id, event_type, controller_token, controller_account_id,
                issued_by_account_id, reason, occurred_at
            )
            VALUES ($1, $2, $3::controller_event_type, $4, $5, $5, $6, NOW())
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(event_type)
        .bind(controller_token)
        .bind(controller_account_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    async fn enqueue_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        actions: Vec<ActionCommand>,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            for action in &actions {
                let payload = serde_json::to_string(action)?;
                redis::cmd("XADD")
                    .arg(&key)
                    .arg("MAXLEN")
                    .arg("~")
                    .arg(REDIS_ACTION_STREAM_MAXLEN)
                    .arg("*")
                    .arg("payload")
                    .arg(payload)
                    .query_async::<_, String>(&mut conn)
                    .await?;
            }
            redis::cmd("EXPIRE")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn drain_actions_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<Vec<ActionCommand>, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let range: Vec<(String, Vec<(String, String)>)> = redis::cmd("XRANGE")
                .arg(&key)
                .arg("-")
                .arg("+")
                .query_async(&mut conn)
                .await?;

            let mut actions = Vec::new();
            for (_id, fields) in range {
                for (field, value) in fields {
                    if field == "payload" {
                        let action: ActionCommand = serde_json::from_str(&value)?;
                        actions.push(action);
                    }
                }
            }

            if !actions.is_empty() {
                redis::cmd("DEL")
                    .arg(&key)
                    .query_async::<_, ()>(&mut conn)
                    .await?;
            }

            return Ok(actions);
        }
        let sessions = self.fallback.sessions.read().await;
        Ok(sessions
            .get(session_id)
            .map(|record| record.pending_actions.iter().cloned().collect())
            .unwrap_or_default())
    }

    async fn pending_actions_count(
        &self,
        private_beach_id: &str,
        session_id: &str,
    ) -> Result<usize, StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_actions_key(private_beach_id, session_id);
            let len: usize = redis::cmd("XLEN")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(0);
            return Ok(len);
        }
        let sessions = self.fallback.sessions.read().await;
        Ok(sessions
            .get(session_id)
            .map(|record| record.pending_actions.len())
            .unwrap_or(0))
    }

    async fn store_health_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        heartbeat: &HealthHeartbeat,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_health_key(private_beach_id, session_id);
            let payload = serde_json::to_string(heartbeat)?;
            redis::cmd("SETEX")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .arg(payload)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }

    async fn store_state_redis(
        &self,
        private_beach_id: &str,
        session_id: &str,
        diff: &StateDiff,
    ) -> Result<(), StateError> {
        if let Some(client) = &self.redis {
            let mut conn = client.get_async_connection().await?;
            let key = redis_state_key(private_beach_id, session_id);
            let payload = serde_json::to_string(diff)?;
            redis::cmd("SETEX")
                .arg(&key)
                .arg(REDIS_TTL_SECONDS)
                .arg(payload)
                .query_async::<_, ()>(&mut conn)
                .await?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct LeaseRow {
    id: Uuid,
    controller_account_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

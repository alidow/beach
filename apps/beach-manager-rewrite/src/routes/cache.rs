use crate::queue::ActionCommand;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct CacheSummary {
    host_session_id: String,
    status: &'static str,
    last_action_id: Option<String>,
    last_state_seq: Option<u64>,
    last_updated_ms: Option<u128>,
}

#[derive(Deserialize)]
pub struct CacheTouchRequest {
    pub host_session_id: String,
    pub action_id: String,
}

#[derive(Deserialize)]
pub struct PublishActionRequest {
    pub host_session_id: String,
    pub controller_session_id: String,
    pub action_id: String,
}

pub async fn cache_for_host(
    State(state): State<AppState>,
    Path(host_session_id): Path<String>,
) -> (StatusCode, Json<CacheSummary>) {
    if let Some(snapshot) = state.snapshot().get(&host_session_id) {
        let last_updated_ms = snapshot
            .last_updated
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis());
        return (
            StatusCode::OK,
            Json(CacheSummary {
                host_session_id: snapshot.host_session_id,
                status: "ok",
                last_action_id: snapshot.last_action_id,
                last_state_seq: snapshot.last_state_seq,
                last_updated_ms,
            }),
        );
    }
    (
        StatusCode::NOT_FOUND,
        Json(CacheSummary {
            host_session_id,
            status: "missing",
            last_action_id: None,
            last_state_seq: None,
            last_updated_ms: None,
        }),
    )
}

pub async fn touch_cache(
    State(state): State<AppState>,
    Json(body): Json<CacheTouchRequest>,
) -> StatusCode {
    state
        .snapshot()
        .update_action(&body.host_session_id, &body.action_id);
    StatusCode::ACCEPTED
}

pub async fn publish_action_smoke(
    State(state): State<AppState>,
    Json(body): Json<PublishActionRequest>,
) -> StatusCode {
    let cmd = ActionCommand {
        id: body.action_id.clone(),
        action_type: "smoke".into(),
        payload: serde_json::json!({
            "host_session_id": body.host_session_id,
            "controller_session_id": body.controller_session_id,
        }),
    };
    state.queue().enqueue_action(cmd).await;
    state
        .snapshot()
        .update_action(&body.host_session_id, &body.action_id);
    StatusCode::ACCEPTED
}

pub fn cache_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/cache/:host_session_id",
            axum::routing::get(cache_for_host),
        )
        .route("/smoke/cache-touch", post(touch_cache))
        .route("/smoke/publish-action", post(publish_action_smoke))
}

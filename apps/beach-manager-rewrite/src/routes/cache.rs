use axum::{extract::Path, Json};
use axum::http::StatusCode;
use serde::Serialize;
use crate::state::AppState;
use axum::extract::State;

#[derive(Serialize)]
pub struct CacheSummary {
    host_session_id: String,
    // Placeholder until real cache snapshot exists
    status: &'static str,
}

pub async fn cache_for_host(
    State(_state): State<AppState>,
    Path(host_session_id): Path<String>,
) -> (StatusCode, Json<CacheSummary>) {
    (
        StatusCode::OK,
        Json(CacheSummary {
            host_session_id,
            status: "ok",
        }),
    )
}

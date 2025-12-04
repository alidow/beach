use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::metrics;
use crate::routes::cache::cache_routes;
use crate::state::AppState;

pub mod cache;

#[derive(Serialize)]
struct DebugAttachResponse {
    started: bool,
    error: Option<String>,
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
    uptime_secs: u64,
    instance_id: String,
}

async fn health() -> &'static str {
    "ok"
}

async fn ready(State(state): State<AppState>) -> Json<ReadyResponse> {
    Json(ReadyResponse {
        status: "ok",
        uptime_secs: state.uptime_secs(),
        instance_id: state.instance_id().to_string(),
    })
}

#[derive(Deserialize)]
struct AttachRequest {
    host_session_id: String,
}

#[derive(Serialize)]
struct AttachResponse {
    manager_instance_id: String,
    assigned_here: bool,
    redirect_to: Option<String>,
    reassigned_from: Option<String>,
    reason: Option<String>,
}

async fn attach(
    State(state): State<AppState>,
    Json(body): Json<AttachRequest>,
) -> (StatusCode, Json<AttachResponse>) {
    if !state.assignment_enabled() {
        metrics::ASSIGNMENT_DECISIONS
            .with_label_values(&["disabled"])
            .inc();
        return (
            StatusCode::OK,
            Json(AttachResponse {
                manager_instance_id: state.instance_id().to_string(),
                assigned_here: true,
                redirect_to: None,
                reassigned_from: None,
                reason: Some("assignment_disabled".into()),
            }),
        );
    }

    match state.assignment().assign_host(&body.host_session_id).await {
        Ok(Some(decision)) => {
            let status = if decision.assigned_here {
                StatusCode::OK
            } else {
                StatusCode::TEMPORARY_REDIRECT
            };
            let label = if decision.assigned_here {
                "self"
            } else {
                "redirect"
            };
            metrics::ASSIGNMENT_DECISIONS
                .with_label_values(&[label])
                .inc();
            info!(
                host_session_id = %body.host_session_id,
                target = %decision.selected.id,
                assigned_here = decision.assigned_here,
                reassigned_from = ?decision.reassigned_from,
                "assignment decided"
            );
            if decision.assigned_here {
                match state.attach_bus_for_host(&body.host_session_id).await {
                    Ok(_) => info!(
                        host_session_id = %body.host_session_id,
                        "rtc bus attach requested and started for host"
                    ),
                    Err(err) => warn!(
                        host_session_id = %body.host_session_id,
                        error = %err,
                        "failed to attach rtc bus for host"
                    ),
                }
            }
            (
                status,
                Json(AttachResponse {
                    manager_instance_id: decision.selected.id.clone(),
                    assigned_here: decision.assigned_here,
                    redirect_to: if decision.assigned_here {
                        None
                    } else {
                        Some(decision.selected.id.clone())
                    },
                    reassigned_from: decision.reassigned_from,
                    reason: if decision.assigned_here {
                        None
                    } else {
                        Some("redirect".into())
                    },
                }),
            )
        }
        Ok(None) => {
            metrics::ASSIGNMENT_DECISIONS
                .with_label_values(&["unavailable"])
                .inc();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(AttachResponse {
                    manager_instance_id: state.instance_id().to_string(),
                    assigned_here: false,
                    redirect_to: None,
                    reassigned_from: None,
                    reason: Some("no_available_manager".into()),
                }),
            )
        }
        Err(err) => {
            metrics::ASSIGNMENT_DECISIONS
                .with_label_values(&["error"])
                .inc();
            warn!(
                host_session_id = %body.host_session_id,
                error = %err,
                "assignment resolution failed"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AttachResponse {
                    manager_instance_id: state.instance_id().to_string(),
                    assigned_here: false,
                    redirect_to: None,
                    reassigned_from: None,
                    reason: Some("assignment_error".into()),
                }),
            )
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .route("/readyz", get(ready))
        .route("/attach", post(attach))
        .route(
            "/debug/attach-bus/:host_session_id",
            post(
                |State(state): State<AppState>,
                 axum::extract::Path(host_session_id): axum::extract::Path<String>| async move {
                    match state.attach_bus_for_host(&host_session_id).await {
                        Ok(_) => Json(DebugAttachResponse {
                            started: true,
                            error: None,
                        }),
                        Err(err) => Json(DebugAttachResponse {
                            started: false,
                            error: Some(err.to_string()),
                        }),
                    }
                },
            ),
        )
        .merge(cache_routes())
        .with_state(state)
}

async fn metrics_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain")],
        metrics::gather(),
    )
}

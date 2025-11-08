use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_core::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::info;

use crate::{metrics, state::AppState};

use super::{ApiError, AuthToken};

pub async fn prometheus_metrics() -> String {
    metrics::export_prometheus()
}

pub async fn stream_state(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_scope(&token, "pb:sessions.read")?;
    let rx = state.subscribe_session(&session_id).await;
    let trace_id: Option<Arc<str>> = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| Arc::<str>::from(value));
    if let Some(id) = trace_id.as_deref() {
        info!(
            target: "trace.sse",
            trace_id = id,
            session_id = %session_id,
            event = "stream_state_subscribed"
        );
    }
    let trace_id_for_map = trace_id.clone();
    let stream = BroadcastStream::new(rx).filter_map(move |msg| {
        let trace_id = trace_id_for_map.clone();
        match msg {
            Ok(crate::state::StreamEvent::State(diff)) => {
                if let Some(id) = trace_id.as_deref() {
                    info!(
                        target: "trace.sse",
                        trace_id = &*id,
                        session_id = %session_id,
                        event = "state",
                        "emitting session diff"
                    );
                }
                let data = serde_json::to_string(&diff).unwrap_or_else(|_| "{}".into());
                Some(Ok(Event::default().event("state").data(data)))
            }
            _ => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub async fn stream_controller_pairings(
    State(state): State<AppState>,
    token: AuthToken,
    Path(controller_session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if !(token.has_scope("pb:control.read") || token.has_scope("pb:sessions.read")) {
        return Err(ApiError::Forbidden("pb:control.read"));
    }
    let rx = state.subscribe_session(&controller_session_id).await;
    let trace_id: Option<Arc<str>> = headers
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| Arc::<str>::from(value));
    if let Some(id) = trace_id.as_deref() {
        info!(
            target: "trace.sse",
            trace_id = id,
            controller_session_id = %controller_session_id,
            event = "stream_controller_pairings_subscribed"
        );
    }
    let trace_id_for_map = trace_id.clone();
    let stream = BroadcastStream::new(rx).filter_map(move |msg| {
        let trace_id = trace_id_for_map.clone();
        match msg {
            Ok(crate::state::StreamEvent::ControllerPairing(event)) => {
                if let Some(id) = trace_id.as_deref() {
                    info!(
                        target: "trace.sse",
                        trace_id = &*id,
                        controller_session_id = %controller_session_id,
                        child_session_id = %event.child_session_id,
                        action = ?event.action,
                        "emitting controller pairing event"
                    );
                }
                let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
                Some(Ok(Event::default().event("controller_pairing").data(data)))
            }
            _ => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn ensure_scope(token: &AuthToken, scope: &'static str) -> Result<(), ApiError> {
    if token.has_scope(scope) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(scope))
    }
}

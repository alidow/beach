use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_core::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::{metrics, state::AppState};

use super::{ApiError, AuthToken};

pub async fn prometheus_metrics() -> String {
    metrics::export_prometheus()
}

pub async fn stream_state(
    State(state): State<AppState>,
    token: AuthToken,
    Path(session_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_scope(&token, "pb:sessions.read")?;
    let rx = state.subscribe_session(&session_id).await;
    let stream = BroadcastStream::new(rx)
        .filter(|msg| matches!(msg, Ok(crate::state::StreamEvent::State(_))))
        .map(|msg| {
            if let Ok(crate::state::StreamEvent::State(diff)) = msg {
                let data = serde_json::to_string(&diff).unwrap_or_else(|_| "{}".into());
                Ok(Event::default().event("state").data(data))
            } else {
                Ok(Event::default().event("noop").data("{}"))
            }
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub async fn stream_controller_pairings(
    State(state): State<AppState>,
    token: AuthToken,
    Path(controller_session_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    ensure_scope(&token, "pb:control.write")?;
    let rx = state.subscribe_session(&controller_session_id).await;
    let stream = BroadcastStream::new(rx)
        .filter_map(|msg| match msg {
            Ok(crate::state::StreamEvent::ControllerPairing(event)) => Some(event),
            _ => None,
        })
        .map(|event| {
            let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
            Ok(Event::default().event("controller_pairing").data(data))
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

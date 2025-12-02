use std::sync::Arc;

use axum::body;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use tower::util::ServiceExt;

use beach_manager_rewrite::assignment::AssignmentService;
use beach_manager_rewrite::persistence::{InMemoryPersistence, PersistenceAdapter};
use beach_manager_rewrite::queue::{ControllerQueue, InMemoryQueue};
use beach_manager_rewrite::routes;
use beach_manager_rewrite::state::AppState;
use manager_sdk::assignment_store::{
    AssignmentStore, InMemoryAssignmentStore, ManagerInstanceRecord,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn build_app_with_assignment(
    instance_id: &str,
    self_capacity: u32,
    other: Option<ManagerInstanceRecord>,
) -> Router {
    let store: Arc<dyn AssignmentStore> = InMemoryAssignmentStore::new();
    if let Some(other) = other {
        futures::executor::block_on(store.upsert_instance(other)).unwrap();
    }
    let assignment =
        AssignmentService::from_store(store, instance_id.to_string(), self_capacity, 10_000);
    let queue: Arc<dyn ControllerQueue> = Arc::new(tokio::sync::Mutex::new(InMemoryQueue::new()));
    let persistence: Arc<dyn PersistenceAdapter> = InMemoryPersistence::new();
    let state = AppState::new(
        instance_id.to_string(),
        true,
        queue,
        persistence,
        assignment,
        None,
    );
    routes::router(state)
}

#[tokio::test]
async fn attach_assigns_to_self_when_only_instance() -> TestResult {
    let app = build_app_with_assignment("mgr-self", 5, None);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/attach")
                .header("content-type", "application/json")
                .body(Body::from(json!({"host_session_id": "host-1"}).to_string()))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body::to_bytes(response.into_body(), 1024 * 64).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(json["manager_instance_id"], "mgr-self");
    assert_eq!(json["assigned_here"], true);
    assert!(json["redirect_to"].is_null());
    Ok(())
}

#[tokio::test]
async fn attach_redirects_when_self_at_capacity() -> TestResult {
    let other = ManagerInstanceRecord {
        id: "mgr-other".into(),
        capacity: 5,
        load: 0,
        heartbeat_at: std::time::SystemTime::now(),
    };
    // capacity 0 forces self to be skipped; other is chosen.
    let app = build_app_with_assignment("mgr-self", 0, Some(other));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/attach")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"host_session_id": "host-redirect"}).to_string(),
                ))?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let body = body::to_bytes(response.into_body(), 1024 * 64).await?;
    let json: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(json["manager_instance_id"], "mgr-other");
    assert_eq!(json["assigned_here"], false);
    assert_eq!(json["redirect_to"], "mgr-other");
    Ok(())
}

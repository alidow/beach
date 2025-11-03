//! Postgres-backed integration test for core manager flows.
//!
//! This test is ignored by default. To run it locally:
//! - Start Postgres (e.g., `docker compose up postgres` from repo root)
//! - Export `DATABASE_URL` to point at the Postgres instance
//! - Optionally export `REDIS_URL` if you want to exercise Redis paths
//! - Run: `cargo test -p beach-manager -- --ignored postgres_sqlx_e2e`

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use beach_buggy::{
    AckStatus, ActionAck, ActionCommand, HarnessType, HealthHeartbeat, RegisterSessionRequest,
    StateDiff,
};
use hyper::body::to_bytes;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

use beach_manager::{routes::build_router, state::AppState};

// Single end-to-end flow against a real Postgres database using the SQLx path.
#[ignore]
#[tokio::test]
async fn postgres_sqlx_e2e() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect to postgres");

    // Apply migrations from crate-local migrations folder.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");

    // Build state with DB; Redis is optional, test covers fallback when absent.
    let state = AppState::with_db(pool.clone());
    let app: Router = build_router(state.clone());

    // Stable IDs for the test
    let session_id = Uuid::new_v4().to_string();
    let private_beach_id = Uuid::new_v4().to_string();

    // Register session via state (bypassing HTTP plumbing here for brevity)
    let register = RegisterSessionRequest {
        session_id: session_id.clone(),
        private_beach_id: private_beach_id.clone(),
        harness_type: HarnessType::TerminalShim,
        capabilities: vec!["terminal_diff_v1".into()],
        location_hint: Some("us-test-1".into()),
        metadata: Some(serde_json::json!({ "tag": "pg-e2e" })),
        version: "0.1.0".into(),
        viewer_passcode: Some("PGPASS".into()),
    };
    let register_resp = state
        .register_session(register)
        .await
        .expect("register session");
    assert!(register_resp.controller_token.is_some());

    // List sessions and assert presence
    let sessions = state
        .list_sessions(&private_beach_id)
        .await
        .expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);

    // Acquire controller with short TTL
    let lease = state
        .acquire_controller(&session_id, Some(5_000), Some("e2e".into()), None)
        .await
        .expect("acquire controller");
    assert!(!lease.controller_token.is_empty());

    // Queue an action and poll it back
    let cmd = ActionCommand {
        id: "pg-e2e-1".into(),
        action_type: "key".into(),
        payload: serde_json::json!({"key": "x"}),
        expires_at: None,
    };
    state
        .queue_actions(
            &session_id,
            &lease.controller_token,
            vec![cmd.clone()],
            None,
        )
        .await
        .expect("queue actions");
    let polled = state.poll_actions(&session_id).await.expect("poll");
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].id, cmd.id);

    // Ack the action
    let ack = ActionAck {
        id: cmd.id.clone(),
        status: AckStatus::Ok,
        applied_at: std::time::SystemTime::now(),
        latency_ms: Some(10),
        error_code: None,
        error_message: None,
    };
    state
        .ack_actions(&session_id, vec![ack], None, false)
        .await
        .expect("ack actions");

    // Record health and state
    let hb = HealthHeartbeat {
        queue_depth: 0,
        cpu_load: Some(0.1),
        memory_bytes: Some(1024),
        degraded: false,
        warnings: vec![],
    };
    state
        .record_health(&session_id, hb)
        .await
        .expect("record health");
    let diff = StateDiff {
        sequence: 1,
        emitted_at: std::time::SystemTime::now(),
        payload: serde_json::json!({"ops": []}),
    };
    state
        .record_state(&session_id, diff, false)
        .await
        .expect("record state");

    // Events should be present
    let events = state.controller_events(&session_id).await.expect("events");
    assert!(!events.is_empty());
}

#[ignore]
#[tokio::test]
async fn postgres_viewer_credentials_for_multi_beach_session() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect to postgres");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");

    let state = AppState::with_db(pool.clone());
    let app: Router = build_router(state.clone());

    let origin_session_id = Uuid::new_v4().to_string();
    let private_beach_a = Uuid::new_v4();
    let private_beach_b = Uuid::new_v4();

    let slug_a = format!("test-a-{}", private_beach_a);
    let slug_b = format!("test-b-{}", private_beach_b);

    for (id, name, slug) in [
        (private_beach_a, "Test Beach A", slug_a),
        (private_beach_b, "Test Beach B", slug_b),
    ] {
        sqlx::query(
            r#"
            INSERT INTO private_beach (id, name, slug)
            VALUES ($1, $2, $3)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(slug)
        .execute(&pool)
        .await
        .expect("insert private beach");
    }

    let register_a = RegisterSessionRequest {
        session_id: origin_session_id.clone(),
        private_beach_id: private_beach_a.to_string(),
        harness_type: HarnessType::TerminalShim,
        capabilities: vec!["terminal_diff_v1".into()],
        location_hint: None,
        metadata: None,
        version: "1.0.0".into(),
        viewer_passcode: Some("PASS-A".into()),
    };
    state
        .register_session(register_a)
        .await
        .expect("register session for beach A");

    let register_b = RegisterSessionRequest {
        session_id: origin_session_id.clone(),
        private_beach_id: private_beach_b.to_string(),
        harness_type: HarnessType::TerminalShim,
        capabilities: vec!["terminal_diff_v1".into()],
        location_hint: None,
        metadata: None,
        version: "1.0.0".into(),
        viewer_passcode: Some("PASS-B".into()),
    };
    state
        .register_session(register_b)
        .await
        .expect("register session for beach B");

    let pass_a = state
        .viewer_passcode(&private_beach_a.to_string(), &origin_session_id)
        .await
        .expect("viewer passcode lookup A")
        .expect("passcode present for beach A");
    assert_eq!(pass_a, "PASS-A");

    let pass_b = state
        .viewer_passcode(&private_beach_b.to_string(), &origin_session_id)
        .await
        .expect("viewer passcode lookup B")
        .expect("passcode present for beach B");
    assert_eq!(pass_b, "PASS-B");

    let uri_a = format!(
        "/private-beaches/{}/sessions/{}/viewer-credential",
        private_beach_a, origin_session_id
    );
    let response_a = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri_a)
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .expect("build request A"),
        )
        .await
        .expect("send viewer credential request A");
    assert_eq!(response_a.status(), StatusCode::OK);
    let body_a = to_bytes(response_a.into_body()).await.expect("read body A");
    let json_a: Value = serde_json::from_slice(&body_a).expect("parse body A");
    assert_eq!(json_a["credential_type"], "viewer_passcode");
    assert_eq!(json_a["credential"], "PASS-A");
    assert_eq!(json_a["session_id"], origin_session_id);
    assert_eq!(json_a["private_beach_id"], private_beach_a.to_string());

    let uri_b = format!(
        "/private-beaches/{}/sessions/{}/viewer-credential",
        private_beach_b, origin_session_id
    );
    let response_b = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri_b)
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .expect("build request B"),
        )
        .await
        .expect("send viewer credential request B");
    assert_eq!(response_b.status(), StatusCode::OK);
    let body_b = to_bytes(response_b.into_body()).await.expect("read body B");
    let json_b: Value = serde_json::from_slice(&body_b).expect("parse body B");
    assert_eq!(json_b["credential_type"], "viewer_passcode");
    assert_eq!(json_b["credential"], "PASS-B");
    assert_eq!(json_b["session_id"], origin_session_id);
    assert_eq!(json_b["private_beach_id"], private_beach_b.to_string());
}

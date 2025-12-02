use axum::Router;
use beach_manager_rewrite::assignment;
use beach_manager_rewrite::config::AppConfig;
use beach_manager_rewrite::metrics;
use beach_manager_rewrite::persistence;
use beach_manager_rewrite::pipeline;
use beach_manager_rewrite::queue;
use beach_manager_rewrite::routes;
use beach_manager_rewrite::state::AppState;
use beach_manager_rewrite::telemetry::init_tracing;
use beach_manager_rewrite::transport_shim;
use tracing::info;

#[tokio::main]
async fn main() {
    let cfg = AppConfig::from_env();
    init_tracing(&cfg.log_filter);

    let queue = queue::build_queue(&cfg.queue_backend, cfg.redis_url.as_deref());
    let persistence = persistence::build_persistence(&cfg);
    let assignment_svc = assignment::AssignmentService::build_assignment_service(&cfg).await;
    let _heartbeat = assignment_svc.spawn_heartbeat(cfg.assignment_heartbeat_ms);
    let bus_adapter = transport_shim::build_bus_adapter(&cfg.bus_mode, &cfg.session_server_base);
    let app_state = AppState::new(
        cfg.manager_instance_id.clone(),
        cfg.assignment_enabled,
        queue.clone(),
        persistence.clone(),
        assignment_svc,
        bus_adapter,
    );
    let _drain = pipeline::start_pipeline(
        queue,
        persistence,
        cfg.queue_batch_size,
        cfg.queue_drain_interval_ms,
    );

    let app: Router = routes::router(app_state);

    info!(
        addr = %cfg.bind_addr,
        instance = %cfg.manager_instance_id,
        queue_backend = ?cfg.queue_backend,
        redis = %cfg.redis_url.as_deref().unwrap_or("unset"),
        database = %cfg.database_url.as_deref().unwrap_or("unset"),
        queue_batch = cfg.queue_batch_size,
        queue_interval_ms = cfg.queue_drain_interval_ms,
        assignment_heartbeat_ms = cfg.assignment_heartbeat_ms,
        assignment_ttl_ms = cfg.assignment_ttl_ms,
        assignment_enabled = cfg.assignment_enabled,
        bus_mode = ?cfg.bus_mode,
        session_base = %cfg.session_server_base,
        "starting beach-manager-rewrite"
    );
    metrics::BOOT_COUNTER.inc();
    let listener = tokio::net::TcpListener::bind(cfg.bind_addr)
        .await
        .expect("bind");
    axum::serve(listener, app.into_make_service())
        .await
        .expect("server");
}

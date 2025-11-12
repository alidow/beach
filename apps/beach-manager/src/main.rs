mod auth;
mod config;
mod fastpath;
mod log_throttle;
mod metrics;
mod routes;
mod state;

use auth::{AuthConfig, AuthContext};
use config::AppConfig;
use routes::build_router;
use sqlx::postgres::PgPoolOptions;
use state::{
    viewer_health_report_interval, AppState, STALE_SESSION_MAX_IDLE, STALE_SESSION_SWEEP_INTERVAL,
};
use std::{net::SocketAddr, path::Path, sync::OnceLock, time::Duration};
use tokio::time::sleep;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{filter::EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = AppConfig::from_env();
    init_tracing(cfg.log_path.as_deref());
    info!(
        target = "beach_manager.config",
        bind_addr = %cfg.bind_addr,
        database_configured = cfg.database_url.is_some(),
        redis_configured = cfg.redis_url.is_some(),
        beach_road_url = %cfg.beach_road_url.as_deref().unwrap_or("unset"),
        public_manager_url = %cfg.public_manager_url.as_deref().unwrap_or("unset"),
        auth_bypass = cfg.auth_bypass,
        stale_session_idle_secs = STALE_SESSION_MAX_IDLE.as_secs(),
        viewer_health_interval_secs = viewer_health_report_interval().as_secs(),
        sweep_interval_secs = STALE_SESSION_SWEEP_INTERVAL.as_secs(),
        log_path = %cfg.log_path.as_deref().unwrap_or("unset")
    );

    let mut state = if let Some(db_url) = &cfg.database_url {
        match PgPoolOptions::new()
            .max_connections(20)
            .connect(db_url)
            .await
        {
            Ok(pool) => {
                if let Err(err) = sqlx::migrate!("./migrations").run(&pool).await {
                    warn!(error = %err, "failed to run database migrations");
                } else {
                    info!("database migrations applied");
                }
                AppState::with_db(pool)
            }
            Err(err) => {
                warn!(error = %err, "failed to connect to database, continuing with in-memory state");
                AppState::new()
            }
        }
    } else {
        info!("DATABASE_URL not set; running in in-memory mode");
        AppState::new()
    };

    let auth_config = AuthConfig {
        jwks_url: cfg.beach_gate_jwks_url.clone(),
        issuer: cfg.beach_gate_issuer.clone(),
        audience: cfg.beach_gate_audience.clone(),
        bypass: cfg.auth_bypass,
        cache_ttl: Duration::from_secs(300),
    };
    if !auth_config.bypass && auth_config.jwks_url.is_none() {
        warn!("authentication bypass disabled but BEACH_GATE_JWKS_URL is not set; token verification will fail");
    }
    state = state.with_auth(AuthContext::new(auth_config));
    state = state.with_integrations(cfg.beach_road_url.clone(), cfg.public_manager_url.clone());
    state = state.with_viewer_tokens(
        cfg.beach_gate_url.clone(),
        cfg.beach_gate_viewer_token.clone(),
    );

    {
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            loop {
                cleanup_state.cleanup_stale_sessions().await;
                sleep(STALE_SESSION_SWEEP_INTERVAL).await;
            }
        });
    }

    if let Some(redis_url) = &cfg.redis_url {
        match redis::Client::open(redis_url.clone()) {
            Ok(client) => match client.get_async_connection().await {
                Ok(mut conn) => {
                    if let Err(err) = redis::cmd("PING").query_async::<_, String>(&mut conn).await {
                        warn!(error = %err, "failed to ping redis; continuing without redis");
                        metrics::REDIS_AVAILABLE.set(0);
                    } else {
                        info!("connected to redis at {}", redis_url);
                        metrics::REDIS_AVAILABLE.set(1);
                        state = state.with_redis(client);
                    }
                }
                Err(err) => {
                    warn!(error = %err, "failed to establish redis connection; continuing without redis");
                    metrics::REDIS_AVAILABLE.set(0);
                }
            },
            Err(err) => {
                warn!(error = %err, "invalid REDIS_URL; continuing without redis integration");
                metrics::REDIS_AVAILABLE.set(0);
            }
        }
    } else {
        warn!("REDIS_URL not set; features requiring Redis will be disabled");
        metrics::REDIS_AVAILABLE.set(0);
    }

    let app = build_router(state);

    let addr: SocketAddr = cfg.bind_addr.parse()?;
    info!("Starting Beach Manager on {addr}");
    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app.into_make_service(),
    )
    .await?;

    Ok(())
}

static FILE_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

fn init_tracing(log_path: Option<&str>) {
    use tracing_subscriber::filter::FilterExt;
    use tracing_subscriber::prelude::*;

    let stdout_directives = std::env::var("BEACH_MANAGER_STDOUT_LOG")
        .or_else(|_| std::env::var("STDOUT_LOG"))
        .or_else(|_| std::env::var("RUST_LOG_STDOUT"))
        .unwrap_or_else(|_| "info".to_string());
    let stdout_filter =
        EnvFilter::try_new(stdout_directives).unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(stdout_filter);

    let registry = tracing_subscriber::registry().with(stdout_layer);

    if let Some(path) = log_path.and_then(|p| {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }) {
        if let Some((writer, guard)) = build_file_writer(path) {
            let _ = FILE_GUARD.set(guard);
            let file_directives = std::env::var("BEACH_MANAGER_FILE_LOG")
                .or_else(|_| std::env::var("FILE_LOG"))
                .or_else(|_| std::env::var("RUST_LOG_FILE"))
                .unwrap_or_else(|_| "trace".to_string());
            let file_filter =
                EnvFilter::try_new(file_directives).unwrap_or_else(|_| EnvFilter::new("trace"));
            let file_layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(file_filter);
            registry.with(file_layer).init();
            return;
        }
    }

    registry.init();
}

fn build_file_writer(
    path: &str,
) -> Option<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    let path = Path::new(path);
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!(
                "beach-manager tracing: failed to create directory {}: {err}",
                parent.display()
            );
            return None;
        }
    }

    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => file,
        Err(err) => {
            eprintln!(
                "beach-manager tracing: failed to open {}: {err}",
                path.display()
            );
            return None;
        }
    };

    Some(tracing_appender::non_blocking(file))
}

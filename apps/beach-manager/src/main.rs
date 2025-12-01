mod auth;
mod config;
mod log_throttle;
mod metrics;
mod publish_token;
mod routes;
mod state;

use auth::{AuthAuthority, AuthConfig, AuthContext};
use config::AppConfig;
use routes::build_router;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use state::{
    viewer_health_report_interval, AppState, STALE_SESSION_MAX_IDLE, STALE_SESSION_SWEEP_INTERVAL,
};
use std::{collections::HashSet, net::SocketAddr, path::Path, sync::OnceLock, time::Duration};
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cfg = AppConfig::from_env();
    init_tracing(cfg.log_path.as_deref());
    let build_id = option_env!("BEACH_BUILD_ID").unwrap_or("dev-unknown");
    info!(
        target = "beach_manager.build",
        build_id, "Beach Manager build"
    );
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
        log_path = %cfg.log_path.as_deref().unwrap_or("unset"),
        controller_strict_gating = cfg.controller_strict_gating
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

    let mut authorities = Vec::new();
    if let Some(url) = cfg.beach_gate_jwks_url.clone() {
        authorities.push(AuthAuthority {
            jwks_url: url,
            issuer: cfg.beach_gate_issuer.clone(),
            audience: cfg.beach_gate_audience.clone(),
        });
    }
    if let Some(url) = cfg.clerk_jwks_url.clone() {
        authorities.push(AuthAuthority {
            jwks_url: url,
            issuer: cfg.clerk_issuer.clone(),
            audience: cfg.clerk_audience.clone(),
        });
    }
    let auth_config = AuthConfig {
        authorities,
        bypass: cfg.auth_bypass,
        cache_ttl: Duration::from_secs(300),
    };
    if !auth_config.bypass && auth_config.authorities.is_empty() {
        warn!(
            "authentication bypass disabled but no JWKS authorities are configured; token verification will fail"
        );
    }
    state = state.with_auth(AuthContext::new(auth_config));
    state = state.with_integrations(cfg.beach_road_url.clone(), cfg.public_manager_url.clone());
    state = state.with_viewer_tokens(
        cfg.beach_gate_url.clone(),
        cfg.beach_gate_viewer_token.clone(),
    );
    state = state.with_gate_client(cfg.beach_gate_url.clone());
    state = state.with_controller_strict_gating(cfg.controller_strict_gating);
    state = state.with_idle_snapshot_interval(cfg.idle_snapshot_interval_ms);

    init_signing_key_monitor(&cfg).await?;

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

#[derive(Clone)]
struct SigningKeyMonitor {
    jwks_url: String,
    signing_kid_path: Option<String>,
    signing_key_endpoint: Option<String>,
    interval_secs: u64,
}

async fn init_signing_key_monitor(cfg: &AppConfig) -> anyhow::Result<()> {
    let Some(jwks_url) = cfg.beach_gate_jwks_url.clone() else {
        return Ok(());
    };
    let monitor = SigningKeyMonitor {
        jwks_url,
        signing_kid_path: cfg.beach_gate_signing_kid_path.clone(),
        signing_key_endpoint: cfg
            .beach_gate_url
            .as_ref()
            .map(|url| format!("{}/signing-key", url.trim_end_matches('/'))),
        interval_secs: cfg.signing_key_check_interval_secs.max(1),
    };

    const MAX_STARTUP_ATTEMPTS: usize = 5;
    let mut attempt = 0usize;
    loop {
        match ensure_matching_signing_key(&monitor).await {
            Ok(kid) => {
                info!(kid = %kid, "verified Beach Gate signing key");
                break;
            }
            Err(err) => {
                attempt += 1;
                if attempt >= MAX_STARTUP_ATTEMPTS {
                    return Err(err);
                }
                warn!(error = %err, attempt, "waiting for Beach Gate signing key");
                sleep(Duration::from_secs(2_u64.pow(attempt as u32))).await;
            }
        }
    }

    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(monitor.interval_secs)).await;
            if let Err(err) = ensure_matching_signing_key(&monitor).await {
                error!(error = %err, "Beach Gate signing key mismatch detected; terminating");
                std::process::exit(1);
            }
        }
    });

    Ok(())
}

async fn ensure_matching_signing_key(monitor: &SigningKeyMonitor) -> anyhow::Result<String> {
    let mut expected: Option<String> = None;
    if let Some(path) = &monitor.signing_kid_path {
        match tokio::fs::read_to_string(path).await {
            Ok(raw) => {
                let kid = raw.trim().to_string();
                if kid.is_empty() {
                    warn!(path, "signing kid file is empty");
                } else {
                    expected = Some(kid);
                }
            }
            Err(err) => {
                warn!(path, error = %err, "failed to read signing kid file");
            }
        }
    }

    if let Some(endpoint) = &monitor.signing_key_endpoint {
        match fetch_signing_key(endpoint).await {
            Ok(http_kid) => {
                if let Some(expected_kid) = &expected {
                    if expected_kid != &http_kid {
                        anyhow::bail!(
                            "signing kid mismatch between file ({expected_kid}) and Beach Gate ({http_kid})"
                        );
                    }
                } else {
                    expected = Some(http_kid);
                }
            }
            Err(err) => {
                warn!(url = %endpoint, error = %err, "failed to fetch signing key metadata");
            }
        }
    }

    let jwks_kids = fetch_jwks_kids(&monitor.jwks_url).await?;
    if let Some(expected_kid) = expected {
        if !jwks_kids.contains(&expected_kid) {
            anyhow::bail!(
                "expected signing kid {expected_kid} not present in JWKS {}",
                monitor.jwks_url
            );
        }
        return Ok(expected_kid);
    }

    if jwks_kids.len() == 1 {
        if let Some(kid) = jwks_kids.into_iter().next() {
            return Ok(kid);
        }
    }

    anyhow::bail!(
        "unable to determine Beach Gate signing key; set BEACH_GATE_SIGNING_KID_PATH or BEACH_GATE_URL"
    );
}

async fn fetch_signing_key(url: &str) -> anyhow::Result<String> {
    let resp = reqwest::get(url).await?.error_for_status()?;
    let body: SigningKeyResponse = resp.json().await?;
    if body.kid.trim().is_empty() {
        anyhow::bail!("empty signing kid returned by {}", url);
    }
    Ok(body.kid.trim().to_string())
}

async fn fetch_jwks_kids(url: &str) -> anyhow::Result<HashSet<String>> {
    let resp = reqwest::get(url).await?.error_for_status()?;
    let body: JwksResponse = resp.json().await?;
    let kids: HashSet<String> = body
        .keys
        .into_iter()
        .map(|key| key.kid)
        .filter(|kid| !kid.trim().is_empty())
        .collect();
    if kids.is_empty() {
        anyhow::bail!("no keys returned by JWKS {}", url);
    }
    Ok(kids)
}

#[derive(Debug, Deserialize)]
struct SigningKeyResponse {
    kid: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kid: String,
}

fn init_tracing(log_path: Option<&str>) {
    use tracing_subscriber::prelude::*;

    let stdout_env = std::env::var("BEACH_MANAGER_STDOUT_LOG")
        .or_else(|_| std::env::var("STDOUT_LOG"))
        .or_else(|_| std::env::var("RUST_LOG_STDOUT"));
    let (stdout_default, stdout_throttled) = default_manager_stdout_filter();
    let mut deps_throttled = false;
    let stdout_directives = match stdout_env {
        Ok(value) => value,
        Err(_) => {
            if stdout_throttled {
                deps_throttled = true;
            }
            stdout_default
        }
    };
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
            let file_env = std::env::var("BEACH_MANAGER_FILE_LOG")
                .or_else(|_| std::env::var("FILE_LOG"))
                .or_else(|_| std::env::var("RUST_LOG_FILE"));
            let (file_default, file_throttled) = default_manager_file_filter();
            let file_directives = match file_env {
                Ok(value) => value,
                Err(_) => {
                    if file_throttled {
                        deps_throttled = true;
                    }
                    file_default
                }
            };
            let file_filter =
                EnvFilter::try_new(file_directives).unwrap_or_else(|_| EnvFilter::new("trace"));
            let file_layer = tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(file_filter);
            registry.with(file_layer).init();
            if deps_throttled {
                eprintln!(
                    "[beach-manager] suppressing dependency trace noise; set BEACH_MANAGER_TRACE_DEPS=1 or BEACH_TRACE_DEPS=1 to restore full traces"
                );
            }
            return;
        }
    }

    registry.init();
    if deps_throttled {
        eprintln!(
            "[beach-manager] suppressing dependency trace noise; set BEACH_MANAGER_TRACE_DEPS=1 or BEACH_TRACE_DEPS=1 to restore full traces"
        );
    }
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

fn env_truthy(var: &str) -> Option<bool> {
    std::env::var(var)
        .ok()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "false" | "off" | "no" => false,
            _ => true,
        })
}

const MANAGER_TRACE_DEP_TARGETS: &[&str] = &[
    "hyper",
    "hyper_util",
    "tokio_tungstenite",
    "tungstenite",
    "reqwest",
    "quinn_proto",
    "rustls",
    "mio",
    "h2",
];

fn default_manager_stdout_filter() -> (String, bool) {
    build_manager_filter("info")
}

fn default_manager_file_filter() -> (String, bool) {
    build_manager_filter("info,beach_manager=trace")
}

fn build_manager_filter(base: &str) -> (String, bool) {
    if allow_dependency_traces() {
        (base.to_owned(), false)
    } else {
        (throttle_dependency_targets(base), true)
    }
}

fn allow_dependency_traces() -> bool {
    env_truthy("BEACH_MANAGER_TRACE_DEPS")
        .or_else(|| env_truthy("BEACH_TRACE_DEPS"))
        .unwrap_or(false)
}

fn throttle_dependency_targets(base: &str) -> String {
    let mut filter = base.to_owned();
    for target in MANAGER_TRACE_DEP_TARGETS {
        filter.push(',');
        filter.push_str(target);
        filter.push_str("=info");
    }
    filter
}

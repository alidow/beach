mod auth;
mod config;
mod fastpath;
mod metrics;
mod routes;
mod state;

use auth::{AuthConfig, AuthContext};
use config::AppConfig;
use routes::build_router;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use std::{net::SocketAddr, time::Duration};
use tracing::{info, warn, Level};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let cfg = AppConfig::from_env();
    let mut state = if let Some(db_url) = &cfg.database_url {
        match PgPoolOptions::new()
            .max_connections(5)
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

fn init_tracing() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

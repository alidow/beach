use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub log_filter: String,
    pub manager_instance_id: String,
    pub queue_backend: QueueBackend,
    pub redis_url: Option<String>,
    pub database_url: Option<String>,
    pub manager_capacity: u32,
    pub queue_batch_size: usize,
    pub queue_drain_interval_ms: u64,
    pub assignment_heartbeat_ms: u64,
    pub assignment_ttl_ms: u64,
    pub assignment_enabled: bool,
    pub bus_mode: BusMode,
    pub session_server_base: String,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let bind_addr: SocketAddr = std::env::var("BEACH_MANAGER_REWRITE_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8081".into())
            .parse()
            .expect("valid addr");
        let log_filter =
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,beach_manager_rewrite=debug".into());
        let manager_instance_id = std::env::var("BEACH_MANAGER_INSTANCE_ID")
            .unwrap_or_else(|_| "manager-rewrite-1".into());
        let queue_backend = QueueBackend::from_env();
        let redis_url = std::env::var("REDIS_URL").ok();
        let database_url = std::env::var("DATABASE_URL").ok();
        let manager_capacity = std::env::var("BEACH_MANAGER_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);
        let queue_batch_size = std::env::var("BEACH_QUEUE_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128);
        let queue_drain_interval_ms = std::env::var("BEACH_QUEUE_DRAIN_INTERVAL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);
        let assignment_heartbeat_ms = std::env::var("BEACH_ASSIGNMENT_HEARTBEAT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2_000);
        let assignment_ttl_ms = std::env::var("BEACH_ASSIGNMENT_TTL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15_000);
        let assignment_enabled = std::env::var("BEACH_ASSIGNMENT_ENABLED")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(true);
        let bus_mode = BusMode::from_env();
        let session_server_base = std::env::var("BEACH_ROAD_URL")
            .or_else(|_| std::env::var("BEACH_SESSION_SERVER_BASE"))
            .unwrap_or_else(|_| "http://api.beach.dev:4132".into());
        Self {
            bind_addr,
            log_filter,
            manager_instance_id,
            queue_backend,
            redis_url,
            database_url,
            manager_capacity,
            queue_batch_size,
            queue_drain_interval_ms,
            assignment_heartbeat_ms,
            assignment_ttl_ms,
            assignment_enabled,
            bus_mode,
            session_server_base,
        }
    }
}

#[derive(Debug, Clone)]
pub enum QueueBackend {
    InMemory,
    Redis,
}

impl QueueBackend {
    fn from_env() -> Self {
        match std::env::var("BEACH_QUEUE_BACKEND")
            .unwrap_or_else(|_| "memory".into())
            .as_str()
        {
            "redis" => QueueBackend::Redis,
            _ => QueueBackend::InMemory,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusMode {
    Disabled,
    Ipc,
    Rtc,
}

impl BusMode {
    fn from_env() -> Self {
        match std::env::var("BEACH_MANAGER_BUS_MODE")
            .unwrap_or_else(|_| "disabled".into())
            .as_str()
        {
            "ipc" => BusMode::Ipc,
            "rtc" => BusMode::Rtc,
            _ => BusMode::Disabled,
        }
    }
}

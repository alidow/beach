use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub redis_url: String,
    pub session_ttl_seconds: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: env::var("BEACH_ROAD_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8080),
            redis_url: env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            session_ttl_seconds: env::var("SESSION_TTL")
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or(2_592_000), // default 30 days
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8080,
            redis_url: "redis://localhost:6379".to_string(),
            session_ttl_seconds: 2_592_000,
        }
    }
}

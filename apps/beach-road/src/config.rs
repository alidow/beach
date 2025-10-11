use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub redis_url: String,
    pub session_ttl_seconds: u64,
    pub fallback_guardrail_threshold: f64,
    pub fallback_token_ttl_seconds: u64,
    pub fallback_require_oidc: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let fallback_guardrail_threshold = env::var("FALLBACK_GUARDRAIL_THRESHOLD")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(0.005); // 0.5%
        let fallback_token_ttl_seconds = env::var("FALLBACK_TOKEN_TTL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(300);
        let fallback_require_oidc = env::var("FALLBACK_REQUIRE_OIDC")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

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
            fallback_guardrail_threshold,
            fallback_token_ttl_seconds,
            fallback_require_oidc,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8080,
            redis_url: "redis://localhost:6379".to_string(),
            session_ttl_seconds: 2_592_000,
            fallback_guardrail_threshold: 0.005,
            fallback_token_ttl_seconds: 300,
            fallback_require_oidc: false,
        }
    }
}

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub redis_url: String,
    pub session_ttl_seconds: u64,
    pub fallback_guardrail_threshold: f64,
    pub fallback_token_ttl_seconds: u64,
    pub fallback_require_oidc: bool,
    pub fallback_paused: bool,
    pub fallback_jwks_url: Option<String>,
    pub fallback_jwt_issuer: Option<String>,
    pub fallback_jwt_audience: Option<String>,
    pub fallback_required_entitlement: String,
    pub fallback_jwks_cache_ttl_seconds: u64,
    pub viewer_token_audience: String,
    pub viewer_token_mac_secret: Option<String>,
    pub viewer_token_jwks_cache_ttl_seconds: u64,
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
        let fallback_paused = env::var("FALLBACK_WS_PAUSED")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let fallback_jwks_url = env::var("BEACH_GATE_JWKS_URL").ok();
        let fallback_jwt_issuer = env::var("BEACH_GATE_ISSUER").ok();
        let fallback_jwt_audience = env::var("BEACH_GATE_AUDIENCE").ok();
        let fallback_required_entitlement =
            env::var("FALLBACK_REQUIRED_ENTITLEMENT").unwrap_or_else(|_| "rescue:fallback".into());
        let fallback_jwks_cache_ttl_seconds = env::var("FALLBACK_JWKS_CACHE_TTL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(300);
        let viewer_token_audience =
            env::var("BEACH_GATE_VIEWER_TOKEN_AUDIENCE").unwrap_or_else(|_| "beach-road".into());
        let viewer_token_mac_secret = env::var("BEACH_GATE_VIEWER_TOKEN_SECRET").ok();
        let viewer_token_jwks_cache_ttl_seconds = env::var("VIEWER_TOKEN_JWKS_CACHE_TTL")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(fallback_jwks_cache_ttl_seconds);

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
            fallback_paused,
            fallback_jwks_url,
            fallback_jwt_issuer,
            fallback_jwt_audience,
            fallback_required_entitlement,
            fallback_jwks_cache_ttl_seconds,
            viewer_token_audience,
            viewer_token_mac_secret,
            viewer_token_jwks_cache_ttl_seconds,
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
            fallback_paused: false,
            fallback_jwks_url: None,
            fallback_jwt_issuer: None,
            fallback_jwt_audience: None,
            fallback_required_entitlement: "rescue:fallback".to_string(),
            fallback_jwks_cache_ttl_seconds: 300,
            viewer_token_audience: "beach-road".to_string(),
            viewer_token_mac_secret: None,
            viewer_token_jwks_cache_ttl_seconds: 300,
        }
    }
}

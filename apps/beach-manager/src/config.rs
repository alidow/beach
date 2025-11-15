use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default)]
    pub database_url: Option<String>,
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub beach_gate_jwks_url: Option<String>,
    #[serde(default)]
    pub beach_gate_issuer: Option<String>,
    #[serde(default)]
    pub beach_gate_audience: Option<String>,
    #[serde(default)]
    pub clerk_jwks_url: Option<String>,
    #[serde(default)]
    pub clerk_issuer: Option<String>,
    #[serde(default)]
    pub clerk_audience: Option<String>,
    #[serde(default)]
    pub beach_gate_url: Option<String>,
    #[serde(default)]
    pub beach_gate_viewer_token: Option<String>,
    #[serde(default)]
    pub beach_gate_signing_kid_path: Option<String>,
    #[serde(default)]
    pub auth_bypass: bool,
    #[serde(default)]
    pub beach_road_url: Option<String>,
    #[serde(default)]
    pub public_manager_url: Option<String>,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default = "default_true")]
    pub controller_strict_gating: bool,
    #[serde(default = "default_idle_snapshot_interval")]
    pub idle_snapshot_interval_ms: Option<u64>,
    #[serde(default = "default_signing_key_check_interval")]
    pub signing_key_check_interval_secs: u64,
}

impl AppConfig {
    pub fn from_env() -> Self {
        config::Config::builder()
            .add_source(config::Environment::default().separator("__"))
            .build()
            .and_then(|c| c.try_deserialize())
            .map(AppConfig::normalize)
            .unwrap_or_else(|_| {
                AppConfig {
                    bind_addr: default_bind_addr(),
                    database_url: None,
                    redis_url: None,
                    beach_gate_jwks_url: None,
                    beach_gate_issuer: None,
                    beach_gate_audience: None,
                    clerk_jwks_url: None,
                    clerk_issuer: None,
                    clerk_audience: None,
                    beach_gate_url: None,
                    beach_gate_viewer_token: None,
                    beach_gate_signing_kid_path: None,
                    auth_bypass: false,
                    beach_road_url: None,
                    public_manager_url: None,
                    log_path: None,
                    controller_strict_gating: true,
                    idle_snapshot_interval_ms: default_idle_snapshot_interval(),
                    signing_key_check_interval_secs: default_signing_key_check_interval(),
                }
                .normalize()
            })
    }

    fn normalize(mut self) -> Self {
        self.database_url = Self::normalize_opt(self.database_url.take());
        self.redis_url = Self::normalize_opt(self.redis_url.take());
        self.beach_gate_jwks_url = Self::normalize_opt(self.beach_gate_jwks_url.take());
        self.beach_gate_issuer = Self::normalize_opt(self.beach_gate_issuer.take());
        self.beach_gate_audience = Self::normalize_opt(self.beach_gate_audience.take());
        self.clerk_jwks_url = Self::normalize_opt(self.clerk_jwks_url.take());
        self.clerk_issuer = Self::normalize_opt(self.clerk_issuer.take());
        self.clerk_audience = Self::normalize_opt(self.clerk_audience.take());
        self.beach_gate_url = Self::normalize_opt(self.beach_gate_url.take());
        self.beach_gate_viewer_token = Self::normalize_opt(self.beach_gate_viewer_token.take());
        self.beach_gate_signing_kid_path =
            Self::normalize_opt(self.beach_gate_signing_kid_path.take());
        self.beach_road_url = Self::normalize_opt(self.beach_road_url.take());
        self.public_manager_url = Self::normalize_opt(self.public_manager_url.take());
        self.log_path = Self::normalize_opt(self.log_path.take());
        self
    }

    fn normalize_opt(value: Option<String>) -> Option<String> {
        value.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_true() -> bool {
    true
}

fn default_idle_snapshot_interval() -> Option<u64> {
    Some(30_000)
}

fn default_signing_key_check_interval() -> u64 {
    60
}

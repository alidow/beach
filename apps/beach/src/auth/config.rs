use crate::auth::error::AuthError;
use std::env;
use url::Url;

const DEFAULT_AUTH_GATEWAY: &str = "https://auth.beach.sh";

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub gateway: Url,
    pub scope: Option<String>,
    pub audience: Option<String>,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self, AuthError> {
        let gateway =
            env::var("BEACH_AUTH_GATEWAY").unwrap_or_else(|_| DEFAULT_AUTH_GATEWAY.to_string());
        let gateway = Url::parse(&gateway)
            .map_err(|err| AuthError::Config(format!("invalid BEACH_AUTH_GATEWAY: {err}")))?;

        let scope = env::var("BEACH_AUTH_SCOPE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let audience = env::var("BEACH_AUTH_AUDIENCE")
            .ok()
            .filter(|s| !s.trim().is_empty());

        Ok(Self {
            gateway,
            scope,
            audience,
        })
    }
}

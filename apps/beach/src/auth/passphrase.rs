use crate::auth::error::AuthError;
use std::env;
use std::sync::OnceLock;

static PASSPHRASE: OnceLock<Option<String>> = OnceLock::new();

pub fn optional_passphrase() -> Result<Option<String>, AuthError> {
    let value = PASSPHRASE.get_or_init(|| {
        env::var("BEACH_AUTH_PASSPHRASE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    Ok(value.clone())
}

pub fn require_passphrase() -> Result<String, AuthError> {
    optional_passphrase()?.ok_or(AuthError::PassphraseMissing)
}

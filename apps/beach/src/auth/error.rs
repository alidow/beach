use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("config error: {0}")]
    Config(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("serialization error: {0}")]
    Toml(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("gateway error: {0}")]
    Gateway(String),
    #[error("keyring error: {0}")]
    Keyring(String),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("Beach Auth profile not found")]
    NotLoggedIn,
    #[error("profile '{0}' not found")]
    ProfileNotFound(String),
    #[error("profile lacks websocket fallback entitlement")]
    FallbackNotEntitled,
    #[error("profile lacks TURN fallback entitlement")]
    TurnNotEntitled,
    #[error("authorization is still pending")]
    AuthorizationPending,
    #[error("authorization request was denied")]
    AuthorizationDenied,
    #[error("passphrase required to unlock Beach Auth credentials")]
    PassphraseMissing,
    #[error("{0}")]
    Other(String),
}

impl From<toml::de::Error> for AuthError {
    fn from(value: toml::de::Error) -> Self {
        AuthError::Toml(value.to_string())
    }
}

impl From<toml::ser::Error> for AuthError {
    fn from(value: toml::ser::Error) -> Self {
        AuthError::Toml(value.to_string())
    }
}

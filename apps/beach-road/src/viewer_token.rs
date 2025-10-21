use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::storage::SessionInfo;

type HmacSha256 = Hmac<sha2::Sha256>;

#[derive(Clone)]
pub struct ViewerTokenVerifier {
    jwks_url: String,
    issuer: Option<String>,
    audience: String,
    cache_ttl: Duration,
    cache: Arc<RwLock<Option<JwksCache>>>,
    client: Client,
    mac_secret: Vec<u8>,
}

struct JwksCache {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

impl JwksCache {
    fn stale(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() > ttl
    }
}

#[derive(Debug, Error)]
pub enum ViewerTokenError {
    #[error("jwks url not configured")]
    MissingJwksUrl,
    #[error("jwks fetch failed: {0}")]
    JwksFetch(String),
    #[error("token header missing kid")]
    MissingKid,
    #[error("unknown jwk key id {0}")]
    UnknownKey(String),
    #[error("token validation failed: {0}")]
    InvalidToken(#[from] jsonwebtoken::errors::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("token missing session identifier")]
    MissingSubject,
    #[error("viewer token mac mismatch")]
    MacMismatch,
    #[error("viewer token session mismatch")]
    SessionMismatch,
    #[error("viewer token missing mac claim")]
    MissingMac,
    #[error("viewer token missing viewer type")]
    MissingViewerType,
}

#[derive(Debug, Deserialize)]
struct ViewerTokenClaims {
    sub: String,
    #[serde(default)]
    mac: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    viewer: Option<bool>,
}

impl ViewerTokenVerifier {
    pub fn new(
        jwks_url: String,
        issuer: Option<String>,
        audience: String,
        cache_ttl: Duration,
        mac_secret: Vec<u8>,
    ) -> Self {
        Self {
            jwks_url,
            issuer,
            audience,
            cache_ttl,
            cache: Arc::new(RwLock::new(None)),
            client: Client::new(),
            mac_secret,
        }
    }

    pub async fn verify(&self, token: &str, session: &SessionInfo) -> Result<(), ViewerTokenError> {
        let header = decode_header(token)?;
        let kid = header.kid.ok_or(ViewerTokenError::MissingKid)?;
        let key = self.decoding_key(&kid).await?;

        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&[self.audience.as_str()]);
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
        }

        let data = decode::<ViewerTokenClaims>(token, &key, &validation)?;
        let claims = data.claims;

        if claims.sub.trim().is_empty() {
            return Err(ViewerTokenError::MissingSubject);
        }

        if claims.sub != session.session_id {
            return Err(ViewerTokenError::SessionMismatch);
        }

        let viewer_type_ok = claims
            .token_type
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("viewer"))
            .unwrap_or(false)
            || claims.viewer.unwrap_or(false);
        if !viewer_type_ok {
            return Err(ViewerTokenError::MissingViewerType);
        }

        let mac_claim = claims.mac.ok_or(ViewerTokenError::MissingMac)?;
        let expected = compute_mac(&self.mac_secret, &session.session_id, &session.join_code);
        if mac_claim != expected {
            return Err(ViewerTokenError::MacMismatch);
        }

        Ok(())
    }

    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, ViewerTokenError> {
        {
            let cache = self.cache.read().await;
            if let Some(cache) = cache.as_ref() {
                if !cache.stale(self.cache_ttl) {
                    if let Some(key) = cache.keys.get(kid) {
                        return Ok(key.clone());
                    }
                }
            }
        }

        {
            let mut cache = self.cache.write().await;
            let refresh_needed = cache
                .as_ref()
                .map(|c| c.stale(self.cache_ttl))
                .unwrap_or(true);
            if refresh_needed {
                *cache = Some(self.fetch_jwks().await?);
            }

            if let Some(cache) = cache.as_ref() {
                if let Some(key) = cache.keys.get(kid) {
                    return Ok(key.clone());
                }
            }
        }

        Err(ViewerTokenError::UnknownKey(kid.to_string()))
    }

    async fn fetch_jwks(&self) -> Result<JwksCache, ViewerTokenError> {
        if self.jwks_url.trim().is_empty() {
            return Err(ViewerTokenError::MissingJwksUrl);
        }

        let response = self.client.get(&self.jwks_url).send().await?;
        if !response.status().is_success() {
            return Err(ViewerTokenError::JwksFetch(format!(
                "status {}",
                response.status()
            )));
        }

        let body: JwksResponse = response.json().await?;
        let mut keys = HashMap::new();
        for entry in body.keys {
            if entry.kty != "EC" {
                continue;
            }
            if let (Some(kid), Some(x), Some(y)) = (entry.kid, entry.x, entry.y) {
                let decoded_x = URL_SAFE_NO_PAD
                    .decode(x)
                    .map_err(|err| ViewerTokenError::JwksFetch(err.to_string()))?;
                let decoded_y = URL_SAFE_NO_PAD
                    .decode(y)
                    .map_err(|err| ViewerTokenError::JwksFetch(err.to_string()))?;
                let key = DecodingKey::from_ec_components(&decoded_x, &decoded_y)?;
                keys.insert(kid, key);
            }
        }

        Ok(JwksCache {
            keys,
            fetched_at: Instant::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kid: Option<String>,
    kty: String,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

fn compute_mac(secret: &[u8], session_id: &str, join_code: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("invalid hmac key");
    mac.update(session_id.as_bytes());
    mac.update(b":");
    mac.update(join_code.as_bytes());
    let result = mac.finalize().into_bytes();
    URL_SAFE_NO_PAD.encode(result)
}

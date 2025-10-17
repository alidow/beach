use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct VerifiedEntitlements {
    pub subject: String,
    pub email: Option<String>,
    pub entitlements: Vec<String>,
    pub tier: Option<String>,
    pub profile: Option<String>,
}

#[derive(Clone)]
pub struct EntitlementVerifier {
    jwks_url: String,
    issuer: Option<String>,
    audience: Option<String>,
    required_entitlement: String,
    cache_ttl: Duration,
    cache: Arc<RwLock<Option<JwksCache>>>,
    client: Client,
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
pub enum EntitlementError {
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
    #[error("token missing subject")]
    MissingSubject,
    #[error("required entitlement '{0}' missing")]
    MissingEntitlement(String),
}

#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    entitlements: Vec<String>,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    profile: Option<String>,
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
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

impl EntitlementVerifier {
    pub fn new(
        jwks_url: String,
        issuer: Option<String>,
        audience: Option<String>,
        required_entitlement: String,
        cache_ttl: Duration,
    ) -> Self {
        Self {
            jwks_url,
            issuer,
            audience,
            required_entitlement,
            cache_ttl,
            cache: Arc::new(RwLock::new(None)),
            client: Client::new(),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<VerifiedEntitlements, EntitlementError> {
        let header = decode_header(token)?;
        let kid = header.kid.ok_or(EntitlementError::MissingKid)?;
        let key = self.decoding_key(&kid).await?;

        let mut validation = Validation::new(Algorithm::ES256);
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
        }
        if let Some(audience) = &self.audience {
            validation.set_audience(&[audience]);
        }

        let data = decode::<AccessTokenClaims>(token, &key, &validation)?;
        let claims = data.claims;

        if claims.sub.trim().is_empty() {
            return Err(EntitlementError::MissingSubject);
        }

        if !claims
            .entitlements
            .iter()
            .any(|ent| ent == &self.required_entitlement)
        {
            return Err(EntitlementError::MissingEntitlement(
                self.required_entitlement.clone(),
            ));
        }

        Ok(VerifiedEntitlements {
            subject: claims.sub,
            email: claims.email,
            entitlements: claims.entitlements,
            tier: claims.tier,
            profile: claims.profile,
        })
    }

    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, EntitlementError> {
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
            let should_refresh = cache
                .as_ref()
                .map(|c| c.stale(self.cache_ttl))
                .unwrap_or(true);
            if should_refresh {
                *cache = Some(self.fetch_jwks().await?);
            }

            if let Some(cache) = cache.as_ref() {
                if let Some(key) = cache.keys.get(kid) {
                    return Ok(key.clone());
                }
            }
        }

        Err(EntitlementError::UnknownKey(kid.to_string()))
    }

    async fn fetch_jwks(&self) -> Result<JwksCache, EntitlementError> {
        if self.jwks_url.trim().is_empty() {
            return Err(EntitlementError::MissingJwksUrl);
        }

        let response = self.client.get(&self.jwks_url).send().await?;
        if !response.status().is_success() {
            return Err(EntitlementError::JwksFetch(format!(
                "status {}",
                response.status()
            )));
        }

        let body: JwksResponse = response.json().await?;
        let mut keys = HashMap::new();
        for entry in body.keys {
            if entry.kty.as_str() != "EC" || entry.crv.as_deref() != Some("P-256") {
                continue;
            }
            let kid = match entry.kid {
                Some(kid) => kid,
                None => continue,
            };
            let x = match entry.x.clone() {
                Some(x) => x,
                None => continue,
            };
            let y = match entry.y.clone() {
                Some(y) => y,
                None => continue,
            };

            match DecodingKey::from_ec_components(&x, &y) {
                Ok(key) => {
                    keys.insert(kid, key);
                }
                Err(err) => {
                    warn!(
                        target: "beach-road::entitlement",
                        error = %err,
                        "failed to parse jwk entry; skipping"
                    );
                }
            }
        }

        if keys.is_empty() {
            return Err(EntitlementError::JwksFetch(
                "no usable keys in JWKS response".to_string(),
            ));
        }

        Ok(JwksCache {
            keys,
            fetched_at: Instant::now(),
        })
    }

    pub fn required_entitlement(&self) -> &str {
        &self.required_entitlement
    }
}

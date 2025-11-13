use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub jwks_url: Option<String>,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub bypass: bool,
    pub cache_ttl: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwks_url: None,
            issuer: None,
            audience: None,
            bypass: false,
            cache_ttl: Duration::from_secs(300),
        }
    }
}

#[derive(Clone)]
pub struct AuthContext {
    config: AuthConfig,
    cache: Arc<RwLock<Option<JwksCache>>>,
    client: reqwest::Client,
}

struct JwksCache {
    keys: HashMap<String, CachedDecodingKey>,
    fetched_at: Instant,
}

impl JwksCache {
    fn stale(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() > ttl
    }
}

#[derive(Clone)]
struct CachedDecodingKey {
    key: DecodingKey,
    algorithm: Algorithm,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("missing bearer token")]
    MissingToken,
    #[error("missing jwks url configuration")]
    MissingJwksConfig,
    #[error("jwt header missing kid")]
    MissingKid,
    #[error("unknown jwk key id {0}")]
    UnknownKey(String),
    #[error("unsupported jwt algorithm {0}")]
    UnsupportedAlgorithm(String),
    #[error("jwt validation failed: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("jwks fetch failed: {0}")]
    JwksFetch(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("payload decode error: {0}")]
    Payload(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub scp: Option<Vec<String>>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub organization_id: Option<String>,
    #[serde(default)]
    pub private_beach_id: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub exp: Option<i64>,
}

#[allow(dead_code)]
impl Claims {
    pub fn scopes(&self) -> Vec<String> {
        let mut scopes = Vec::new();
        if let Some(value) = &self.scope {
            scopes.extend(value.split_whitespace().map(|s| s.to_string()));
        }
        if let Some(values) = &self.scp {
            scopes.extend(values.clone());
        }
        scopes
    }
}

impl AuthContext {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            cache: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Claims, AuthError> {
        self.verify_with_mode(token, self.config.bypass).await
    }

    pub async fn verify_strict(&self, token: &str) -> Result<Claims, AuthError> {
        self.verify_with_mode(token, false).await
    }

    async fn verify_with_mode(&self, token: &str, allow_bypass: bool) -> Result<Claims, AuthError> {
        if token.is_empty() {
            return Err(AuthError::MissingToken);
        }

        if allow_bypass && self.config.bypass {
            return self.decode_without_verification(token);
        }
        let header = decode_header(token)?;
        let kid = header.kid.ok_or(AuthError::MissingKid)?;
        let key = self.decoding_key(&kid).await?;
        let algorithm = select_algorithm(header.alg, key.algorithm)?;

        let mut validation = Validation::new(algorithm);
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(&[issuer]);
        }
        if let Some(audience) = &self.config.audience {
            validation.set_audience(&[audience]);
        }
        let data = decode::<Claims>(token, &key.key, &validation)?;
        Ok(data.claims)
    }

    async fn decoding_key(&self, kid: &str) -> Result<CachedDecodingKey, AuthError> {
        {
            let cache = self.cache.read().await;
            if let Some(cache) = cache.as_ref() {
                if !cache.stale(self.config.cache_ttl) {
                    if let Some(key) = cache.keys.get(kid) {
                        return Ok(key.clone());
                    }
                }
            }
        }

        let mut cache = self.cache.write().await;
        let needs_refresh = cache
            .as_ref()
            .map(|c| c.stale(self.config.cache_ttl) || !c.keys.contains_key(kid))
            .unwrap_or(true);
        if needs_refresh {
            *cache = Some(self.fetch_jwks().await?);
        }

        if let Some(cache) = cache.as_ref() {
            if let Some(key) = cache.keys.get(kid) {
                return Ok(key.clone());
            }
        }

        Err(AuthError::UnknownKey(kid.to_string()))
    }

    async fn fetch_jwks(&self) -> Result<JwksCache, AuthError> {
        let url = self
            .config
            .jwks_url
            .clone()
            .ok_or(AuthError::MissingJwksConfig)?;
        let resp = self.client.get(url).send().await?;
        let resp = resp.error_for_status().map_err(|err| {
            AuthError::JwksFetch(format!("status: {}", err.status().unwrap_or_default()))
        })?;
        let body: JwksResponse = resp.json().await?;
        let mut keys = HashMap::new();
        for key in body.keys {
            let Jwk {
                kid,
                kty,
                n,
                e,
                x,
                y,
                crv,
            } = key;

            match kty.as_str() {
                "RSA" => {
                    let (Some(n), Some(e)) = (n, e) else {
                        continue;
                    };
                    let decoding_key = DecodingKey::from_rsa_components(&n, &e)?;
                    keys.insert(
                        kid,
                        CachedDecodingKey {
                            key: decoding_key,
                            algorithm: Algorithm::RS256,
                        },
                    );
                }
                "EC" => {
                    if crv.as_deref() != Some("P-256") {
                        continue;
                    }
                    let (Some(x), Some(y)) = (x, y) else {
                        continue;
                    };
                    let decoding_key = DecodingKey::from_ec_components(&x, &y)?;
                    keys.insert(
                        kid,
                        CachedDecodingKey {
                            key: decoding_key,
                            algorithm: Algorithm::ES256,
                        },
                    );
                }
                _ => continue,
            }
        }
        if keys.is_empty() {
            return Err(AuthError::JwksFetch("no usable keys returned".into()));
        }
        Ok(JwksCache {
            keys,
            fetched_at: Instant::now(),
        })
    }

    fn decode_without_verification(&self, token: &str) -> Result<Claims, AuthError> {
        match Self::decode_payload(token) {
            Ok(mut claims) => {
                claims.scope = Some("*".into());
                claims.scp = Some(vec!["*".into()]);
                Ok(claims)
            }
            Err(_) => Ok(Claims {
                sub: "auth-bypass".into(),
                iss: None,
                aud: None,
                scope: Some("*".into()),
                scp: Some(vec!["*".into()]),
                account_id: None,
                organization_id: None,
                private_beach_id: None,
                roles: vec!["bypass".into()],
                exp: None,
            }),
        }
    }

    fn decode_payload(token: &str) -> Result<Claims, AuthError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return Err(AuthError::Payload("token missing payload".into()));
        }
        let payload = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|err| AuthError::Payload(err.to_string()))?;
        let claims: Claims =
            serde_json::from_slice(&payload).map_err(|err| AuthError::Payload(err.to_string()))?;
        Ok(claims)
    }
}

fn select_algorithm(header_alg: Algorithm, key_alg: Algorithm) -> Result<Algorithm, AuthError> {
    match header_alg {
        Algorithm::RS256 | Algorithm::ES256 if header_alg == key_alg => Ok(header_alg),
        Algorithm::RS256 | Algorithm::ES256 => Err(AuthError::UnsupportedAlgorithm(format!(
            "{header_alg:?} (expected {key_alg:?})"
        ))),
        other => Err(AuthError::UnsupportedAlgorithm(format!("{other:?}"))),
    }
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    x: Option<String>,
    y: Option<String>,
    crv: Option<String>,
}

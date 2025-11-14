use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_SCOPES: [&str; 2] = ["pb:sessions.write", "pb:harness.publish"];

#[derive(Clone)]
pub struct PublishTokenManager {
    enc: EncodingKey,
    dec: DecodingKey,
    ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct SignedPublishToken {
    pub token: String,
    /// Expiration timestamp (seconds since epoch)
    pub expires_at: i64,
    pub scopes: Vec<String>,
}

#[derive(Debug, Error)]
pub enum PublishTokenError {
    #[error("token missing or malformed")]
    Malformed,
    #[error("token verification failed: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("token sid mismatch")]
    SidMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishClaims {
    pub sid: String,
    pub exp: i64,
    #[serde(default)]
    pub scp: Option<Vec<String>>, // optional scopes for future use
}

impl PublishTokenManager {
    pub fn from_env() -> Self {
        let secret = std::env::var("PUBLISH_TOKEN_SECRET").unwrap_or_else(|_| {
            // Best-effort ephemeral secret; suitable for dev/test
            // 32 random bytes hex-encoded
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut bytes);
            hex::encode(bytes)
        });
        let enc = EncodingKey::from_secret(secret.as_bytes());
        let dec = DecodingKey::from_secret(secret.as_bytes());
        // Default TTL: 30 minutes
        let ttl = Duration::minutes(30);
        Self { enc, dec, ttl }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn sign_for_session(&self, session_id: &str) -> SignedPublishToken {
        let exp = (Utc::now() + self.ttl).timestamp();
        let scopes: Vec<String> = DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect();
        let claims = PublishClaims {
            sid: session_id.to_string(),
            exp,
            scp: Some(scopes.clone()),
        };
        let header = Header::new(Algorithm::HS256);
        let token = jsonwebtoken::encode(&header, &claims, &self.enc).expect("sign publish token");
        SignedPublishToken {
            token,
            expires_at: exp,
            scopes,
        }
    }

    pub fn verify_for_session(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<PublishClaims, PublishTokenError> {
        if token.trim().is_empty() {
            return Err(PublishTokenError::Malformed);
        }
        let mut validation = Validation::new(Algorithm::HS256);
        // `exp` will be validated by default
        validation.validate_exp = true;
        let data = jsonwebtoken::decode::<PublishClaims>(token, &self.dec, &validation)?;
        let claims = data.claims;
        if claims.sid != session_id {
            return Err(PublishTokenError::SidMismatch);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn sign_and_verify_round_trip() {
        let mgr = PublishTokenManager::from_env().with_ttl(Duration::minutes(1));
        let signed = mgr.sign_for_session("sess-123");
        assert!(signed.expires_at > 0);
        assert_eq!(signed.scopes, DEFAULT_SCOPES);
        let claims = mgr
            .verify_for_session(&signed.token, "sess-123")
            .expect("verify");
        assert_eq!(claims.sid, "sess-123");
    }

    #[test]
    fn sid_mismatch_rejected() {
        let mgr = PublishTokenManager::from_env().with_ttl(Duration::minutes(1));
        let signed = mgr.sign_for_session("sess-abc");
        let err = mgr
            .verify_for_session(&signed.token, "sess-def")
            .unwrap_err();
        matches!(err, PublishTokenError::SidMismatch);
    }
}

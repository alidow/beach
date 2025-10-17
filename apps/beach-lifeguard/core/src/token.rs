use crate::CohortId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Preference specified by the client for telemetry/analytics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryPreference {
    Enabled,
    Disabled,
}

impl Default for TelemetryPreference {
    fn default() -> Self {
        TelemetryPreference::Disabled
    }
}

/// Bit-field describing optional token capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenFeatureBits {
    #[serde(default)]
    pub telemetry_enabled: bool,
}

impl Default for TokenFeatureBits {
    fn default() -> Self {
        Self {
            telemetry_enabled: false,
        }
    }
}

impl TokenFeatureBits {
    pub fn with_telemetry(mut self, enabled: bool) -> Self {
        self.telemetry_enabled = enabled;
        self
    }
}

/// Claims encoded inside the Ed25519-signed fallback token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackTokenClaims {
    pub session_id: Uuid,
    pub cohort_id: CohortId,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    #[serde(default)]
    pub feature_bits: TokenFeatureBits,
}

impl FallbackTokenClaims {
    /// Builds a new claims object with the provided TTL.
    pub fn new(
        session_id: Uuid,
        cohort_id: CohortId,
        ttl: Duration,
        telemetry: TelemetryPreference,
    ) -> Self {
        let issued_at = OffsetDateTime::now_utc();
        let expires_at = issued_at + ttl;
        let feature_bits = TokenFeatureBits::default()
            .with_telemetry(matches!(telemetry, TelemetryPreference::Enabled));
        Self {
            session_id,
            cohort_id,
            issued_at,
            expires_at,
            feature_bits,
        }
    }

    /// Returns `Ok(())` if token has not yet expired.
    pub fn ensure_not_expired(&self, now: OffsetDateTime) -> Result<(), TokenValidationError> {
        if now > self.expires_at {
            Err(TokenValidationError::Expired)
        } else {
            Ok(())
        }
    }
}

/// Errors returned while validating a fallback token.
#[derive(Debug, Error)]
pub enum TokenValidationError {
    #[error("token has expired")]
    Expired,
}

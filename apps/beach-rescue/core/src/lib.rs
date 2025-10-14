//! Core primitives shared by the `beach-rescue` transport implementation.
//!
//! This crate intentionally keeps dependencies light so that both the server
//! implementation and any local harnesses can consume a single source of
//! truth for token claims, guardrail accounting, and feature negotiation.

pub mod guardrail;
pub mod token;

pub use guardrail::{GuardrailCounters, GuardrailSnapshot, SoftGuardrailState};
pub use token::{FallbackTokenClaims, TelemetryPreference, TokenFeatureBits, TokenValidationError};

/// Identifier representing a cohort or entitlement group.
///
/// Today this is a thin wrapper over `String`, but wiring it through a newtype
/// keeps type-safety at call sites and makes future migrations (e.g. numeric
/// IDs) easier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CohortId(pub String);

impl From<&str> for CohortId {
    fn from(value: &str) -> Self {
        CohortId(value.to_owned())
    }
}

impl From<String> for CohortId {
    fn from(value: String) -> Self {
        CohortId(value)
    }
}

impl std::fmt::Display for CohortId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Utility that normalises telemetry opt-in/out semantics down to a single
/// boolean for downstream logging.
pub fn is_telemetry_enabled(preference: TelemetryPreference) -> bool {
    matches!(preference, TelemetryPreference::Enabled)
}

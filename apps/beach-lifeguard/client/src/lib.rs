//! Utilities for constructing client-side messages used by the `beach-lifeguard`
//! WebSocket fallback transport. These helpers keep the eventual web/CLI
//! implementations in sync without copying message shapes across crates.

use beach_lifeguard_core::{CohortId, FallbackTokenClaims, TelemetryPreference, TokenFeatureBits};
use serde::{Deserialize, Serialize};
use time::Duration;
use uuid::Uuid;

/// Initial client hello payload sent when opening the fallback WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    pub session_id: Uuid,
    pub protocol_version: u16,
    pub compression: CompressionStrategy,
    pub telemetry: TelemetryPreference,
}

impl ClientHello {
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            protocol_version: 1,
            compression: CompressionStrategy::None,
            telemetry: TelemetryPreference::Disabled,
        }
    }

    pub fn with_compression(mut self, compression: CompressionStrategy) -> Self {
        self.compression = compression;
        self
    }

    pub fn with_telemetry(mut self, preference: TelemetryPreference) -> Self {
        self.telemetry = preference;
        self
    }
}

/// Placeholder compression strategies we plan to negotiate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CompressionStrategy {
    None,
    Brotli,
}

impl Default for CompressionStrategy {
    fn default() -> Self {
        CompressionStrategy::None
    }
}

/// Helper for minting a new token claims object for test/dev usage.
pub fn issue_ephemeral_token(
    cohort: CohortId,
    telemetry: TelemetryPreference,
) -> FallbackTokenClaims {
    let ttl = Duration::minutes(5);
    FallbackTokenClaims::new(Uuid::new_v4(), cohort, ttl, telemetry, false)
}

/// Feature summary returned by the server after the handshake completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    pub accepted_compression: CompressionStrategy,
    pub feature_bits: TokenFeatureBits,
}

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Snapshot of guardrail counters captured from Redis.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuardrailCounters {
    pub total_sessions: u64,
    pub fallback_sessions: u64,
}

impl GuardrailCounters {
    /// Percentage (0.0 - 1.0) of sessions that fell back to the WebSocket path.
    pub fn fallback_ratio(&self) -> f64 {
        if self.total_sessions == 0 {
            return 0.0;
        }
        self.fallback_sessions as f64 / self.total_sessions as f64
    }

    /// Evaluates the soft guardrail threshold, returning the resulting state.
    pub fn evaluate(&self, threshold: f64) -> SoftGuardrailState {
        if self.fallback_ratio() >= threshold {
            SoftGuardrailState::Breaching
        } else {
            SoftGuardrailState::WithinBudget
        }
    }
}

/// Runnable view combining counters with their observation timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailSnapshot {
    pub observed_at: OffsetDateTime,
    pub counters: GuardrailCounters,
}

impl GuardrailSnapshot {
    pub fn new(observed_at: OffsetDateTime, counters: GuardrailCounters) -> Self {
        Self {
            observed_at,
            counters,
        }
    }

    /// Determines whether the snapshot indicates a soft-breach for the given threshold.
    pub fn soft_state(&self, threshold: f64) -> SoftGuardrailState {
        self.counters.evaluate(threshold)
    }
}

/// Soft guardrail state derived from the hourly counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftGuardrailState {
    WithinBudget,
    Breaching,
}

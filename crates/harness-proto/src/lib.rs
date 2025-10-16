//! Shared protocol definitions for manager ↔ harness communication.
//! Keeping this in a dedicated crate allows regeneration of bindings
//! for TypeScript/Go/etc. without pulling in heavier runtime code.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerLease {
    pub controller_token: Uuid,
    pub controller_account_id: Uuid,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDescriptor {
    pub session_id: Uuid,
    pub private_beach_id: Uuid,
    pub harness_type: String,
    pub capabilities: Vec<String>,
}

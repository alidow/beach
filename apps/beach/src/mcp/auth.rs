use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseScope {
    Input,
}

#[derive(Clone, Debug, Serialize)]
pub struct LeaseInfo {
    #[serde(serialize_with = "serde_uuid::serialize")]
    pub lease_id: Uuid,
    pub session_id: String,
    #[serde(serialize_with = "serde_instant::serialize")]
    pub issued_at: SystemTime,
    #[serde(serialize_with = "serde_instant::serialize")]
    pub expires_at: SystemTime,
    pub scope: LeaseScope,
}

#[derive(Debug)]
struct LeaseRecord {
    info: LeaseInfo,
    deadline: Instant,
}

#[derive(Debug)]
pub enum LeaseError {
    ReadOnly,
    AlreadyHeld,
    NotFound,
    Expired,
    InvalidScope,
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseError::ReadOnly => write!(f, "write tools disabled in read-only mode"),
            LeaseError::AlreadyHeld => write!(f, "another client already holds the lease"),
            LeaseError::NotFound => write!(f, "lease not found"),
            LeaseError::Expired => write!(f, "lease expired"),
            LeaseError::InvalidScope => write!(f, "lease scope mismatch"),
        }
    }
}

impl std::error::Error for LeaseError {}

pub struct LeaseManager {
    read_only: bool,
    leases: RwLock<HashMap<Uuid, LeaseRecord>>,
    by_session: RwLock<HashMap<(String, LeaseScope), Uuid>>,
}

impl LeaseManager {
    pub fn new(read_only: bool) -> Self {
        Self {
            read_only,
            leases: RwLock::new(HashMap::new()),
            by_session: RwLock::new(HashMap::new()),
        }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn acquire(
        &self,
        session_id: &str,
        scope: LeaseScope,
        ttl: Duration,
    ) -> Result<LeaseInfo, LeaseError> {
        if self.read_only {
            return Err(LeaseError::ReadOnly);
        }

        self.prune_expired();

        {
            let map = self.by_session.read().unwrap();
            if map.contains_key(&(session_id.to_string(), scope)) {
                return Err(LeaseError::AlreadyHeld);
            }
        }

        let lease_id = Uuid::new_v4();
        let issued_at = SystemTime::now();
        let expires_at = issued_at + ttl;
        let deadline = Instant::now() + ttl;
        let info = LeaseInfo {
            lease_id,
            session_id: session_id.to_string(),
            issued_at,
            expires_at,
            scope,
        };
        let record = LeaseRecord {
            info: info.clone(),
            deadline,
        };

        {
            let mut map = self.leases.write().unwrap();
            map.insert(lease_id, record);
        }
        {
            let mut map = self.by_session.write().unwrap();
            map.insert((session_id.to_string(), scope), lease_id);
        }

        Ok(info)
    }

    pub fn release(&self, lease_id: Uuid) -> Result<(), LeaseError> {
        let record = {
            let mut leases = self.leases.write().unwrap();
            leases.remove(&lease_id)
        }
        .ok_or(LeaseError::NotFound)?;

        let mut by_session = self.by_session.write().unwrap();
        by_session.remove(&(record.info.session_id.clone(), record.info.scope));
        Ok(())
    }

    pub fn validate(
        &self,
        session_id: &str,
        scope: LeaseScope,
        lease_id: Option<Uuid>,
    ) -> Result<(), LeaseError> {
        if self.read_only {
            return Err(LeaseError::ReadOnly);
        }
        let lease_id = lease_id.ok_or(LeaseError::NotFound)?;
        self.prune_expired();
        let leases = self.leases.read().unwrap();
        let record = leases.get(&lease_id).ok_or(LeaseError::NotFound)?;
        if record.info.session_id != session_id {
            return Err(LeaseError::InvalidScope);
        }
        if record.info.scope != scope {
            return Err(LeaseError::InvalidScope);
        }
        if record.deadline <= Instant::now() {
            drop(leases);
            self.release(lease_id)?;
            return Err(LeaseError::Expired);
        }
        Ok(())
    }

    pub fn renew(&self, lease_id: Uuid, ttl: Duration) -> Result<LeaseInfo, LeaseError> {
        if self.read_only {
            return Err(LeaseError::ReadOnly);
        }
        self.prune_expired();
        let mut leases = self.leases.write().unwrap();
        let record = leases.get_mut(&lease_id).ok_or(LeaseError::NotFound)?;
        record.deadline = Instant::now() + ttl;
        record.info.expires_at = SystemTime::now() + ttl;
        Ok(record.info.clone())
    }

    fn prune_expired(&self) {
        let now = Instant::now();
        let mut expired = Vec::new();
        {
            let leases = self.leases.read().unwrap();
            for (id, record) in leases.iter() {
                if record.deadline <= now {
                    expired.push(*id);
                }
            }
        }
        if expired.is_empty() {
            return;
        }
        let mut leases = self.leases.write().unwrap();
        let mut by_session = self.by_session.write().unwrap();
        for id in expired {
            if let Some(record) = leases.remove(&id) {
                by_session.remove(&(record.info.session_id, record.info.scope));
            }
        }
    }
}

mod serde_instant {
    use serde::{self, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(instant: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = instant
            .duration_since(UNIX_EPOCH)
            .map_err(|err| serde::ser::Error::custom(err.to_string()))?;
        let millis = duration.as_millis();
        serializer.serialize_u128(millis)
    }
}

mod serde_uuid {
    use serde::Serializer;
    use uuid::Uuid;

    pub fn serialize<S>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&uuid.to_string())
    }
}

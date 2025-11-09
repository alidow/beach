use crate::auth::config::AuthConfig;
use crate::auth::crypto::{self, EncryptedBlob};
use crate::auth::error::AuthError;
use crate::auth::passphrase;
use directories::BaseDirs;
use keyring::Entry;
use std::env;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};
use url::Url;

const KEYRING_SERVICE: &str = "beach-auth";
pub const FALLBACK_ENTITLEMENT: &str = "rescue:fallback";
pub const TURN_ENTITLEMENT: &str = "private-beach:turn";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenCache {
    pub token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(default)]
    pub entitlements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshTokenRecord {
    Keyring { service: String, account: String },
    /// Plaintext token stored directly in the credentials file (0600 perms).
    /// Matches the common pattern used by many CLIs (aws, gcloud, azure).
    Plain { token: String },
    Encrypted { blob: EncryptedBlob },
}

impl RefreshTokenRecord {
    pub fn read(&self) -> Result<String, AuthError> {
        match self {
            RefreshTokenRecord::Keyring { service, account } => {
                // Respect opt-out: if keychain use is not explicitly enabled,
                // avoid touching the OS keychain to prevent a macOS prompt.
                let use_keychain = env::var("BEACH_AUTH_USE_KEYCHAIN")
                    .map(|v| matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    ))
                    .unwrap_or(false);
                if !use_keychain {
                    return Err(AuthError::Keyring(
                        "keychain access disabled via BEACH_AUTH_USE_KEYCHAIN=false; re-login required".into(),
                    ));
                }
                let entry = Entry::new(service, account)
                    .map_err(|err| AuthError::Keyring(err.to_string()))?;
                entry
                    .get_password()
                    .map_err(|err| AuthError::Keyring(err.to_string()))
            }
            RefreshTokenRecord::Plain { token } => Ok(token.clone()),
            RefreshTokenRecord::Encrypted { blob } => {
                let passphrase = passphrase::require_passphrase()?;
                crypto::decrypt(&passphrase, blob)
            }
        }
    }

    pub fn write(profile: &str, gateway: &Url, token: &str) -> Result<Self, AuthError> {
        // Storage policy (in order):
        // 1) If BEACH_AUTH_USE_KEYCHAIN is truthy -> use OS keychain
        // 2) Else if BEACH_AUTH_PASSPHRASE is set -> store encrypted blob
        // 3) Else -> store plaintext in the credentials file (0600 perms)

        fn env_truthy(name: &str) -> bool {
            env::var(name)
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
        }

        if env_truthy("BEACH_AUTH_USE_KEYCHAIN") {
            let host = gateway.host_str().unwrap_or("beach");
            let account = format!("{profile}@{host}");
            match Entry::new(KEYRING_SERVICE, &account) {
                Ok(entry) => {
                    entry
                        .set_password(token)
                        .map_err(|err| AuthError::Keyring(err.to_string()))?;
                    return Ok(RefreshTokenRecord::Keyring {
                        service: KEYRING_SERVICE.to_string(),
                        account,
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        target: "beach::auth",
                        error = %err,
                        "keyring unavailable; falling back to passphrase/plaintext storage"
                    );
                }
            }
        }

        if let Ok(Some(pass)) = passphrase::optional_passphrase() {
            let blob = crypto::encrypt(&pass, token)?;
            return Ok(RefreshTokenRecord::Encrypted { blob });
        }

        Ok(RefreshTokenRecord::Plain { token: token.to_string() })
    }

    pub fn delete(&self) {
        match self {
            RefreshTokenRecord::Keyring { service, account } => {
                if let Ok(entry) = Entry::new(service, account) {
                    if let Err(err) = entry.delete_password() {
                        tracing::warn!(
                            target: "beach::auth",
                            error = %err,
                            service = %service,
                            account = %account,
                            "failed to delete keyring entry"
                        );
                    }
                }
            }
            RefreshTokenRecord::Plain { .. } | RefreshTokenRecord::Encrypted { .. } => {
                // nothing to delete from external stores
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProfile {
    pub issuer: String,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub beach_profile: Option<String>,
    pub refresh: RefreshTokenRecord,
    #[serde(default)]
    pub entitlements: Vec<String>,
    #[serde(default)]
    pub access_token: Option<AccessTokenCache>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl StoredProfile {
    pub fn refresh_token(&self) -> Result<String, AuthError> {
        self.refresh.read()
    }

    pub fn has_entitlement(&self, entitlement: &str) -> bool {
        self.entitlements.iter().any(|ent| ent == entitlement)
    }

    pub fn has_fallback_entitlement(&self) -> bool {
        self.has_entitlement(FALLBACK_ENTITLEMENT)
    }

    pub fn has_turn_entitlement(&self) -> bool {
        self.has_entitlement(TURN_ENTITLEMENT)
    }

    pub fn cache_access_token(
        &mut self,
        token: String,
        expires_in_seconds: u64,
        entitlements: Vec<String>,
    ) {
        let expires_at = OffsetDateTime::now_utc() + Duration::seconds(expires_in_seconds as i64);
        self.access_token = Some(AccessTokenCache {
            token,
            expires_at,
            entitlements,
        });
        self.updated_at = OffsetDateTime::now_utc();
    }

    pub fn clear_access_token(&mut self) {
        self.access_token = None;
        self.updated_at = OffsetDateTime::now_utc();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CredentialsStore {
    #[serde(default)]
    pub current_profile: Option<String>,
    #[serde(default)]
    pub profiles: HashMap<String, StoredProfile>,
}

impl CredentialsStore {
    pub fn path() -> Result<PathBuf, AuthError> {
        let base = BaseDirs::new()
            .ok_or_else(|| AuthError::Config("unable to determine home directory".into()))?;
        let dir = base.home_dir().join(".beach");
        Ok(dir.join("credentials"))
    }

    pub fn load() -> Result<Self, AuthError> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(CredentialsStore::default());
        }

        let raw = fs::read_to_string(&path)?;
        let mut store: CredentialsStore = toml::from_str(&raw)?;
        store.compact();
        Ok(store)
    }

    pub fn save(&self) -> Result<(), AuthError> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let serialized = toml::to_string_pretty(self)?;
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(serialized.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = file.metadata()?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }
        Ok(())
    }

    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn profile(&self, name: &str) -> Option<&StoredProfile> {
        self.profiles.get(name)
    }

    pub fn profile_mut(&mut self, name: &str) -> Option<&mut StoredProfile> {
        self.profiles.get_mut(name)
    }

    pub fn upsert_profile(&mut self, name: String, profile: StoredProfile, set_current: bool) {
        self.profiles.insert(name.clone(), profile);
        if set_current || self.current_profile.is_none() {
            self.current_profile = Some(name);
        }
    }

    pub fn remove_profile(&mut self, name: &str) -> Option<StoredProfile> {
        let removed = self.profiles.remove(name);
        if let Some(removed_profile) = &removed {
            removed_profile.refresh.delete();
        }
        if self
            .current_profile
            .as_ref()
            .map(|current| current == name)
            .unwrap_or(false)
        {
            self.current_profile = self.profiles.keys().next().cloned();
        }
        removed
    }

    pub fn set_current_profile(&mut self, name: Option<String>) -> Result<(), AuthError> {
        if let Some(name_ref) = name.as_ref() {
            if !self.profiles.contains_key(name_ref) {
                return Err(AuthError::ProfileNotFound(name_ref.clone()));
            }
        }
        self.current_profile = name;
        Ok(())
    }

    pub fn ensure_profile_environment(&self, override_profile: Option<&str>) -> Option<String> {
        if let Some(name) = override_profile {
            return Some(name.to_string());
        }
        if let Ok(env_profile) = std::env::var("BEACH_PROFILE") {
            if !env_profile.trim().is_empty() {
                return Some(env_profile);
            }
        }
        self.current_profile.clone()
    }

    pub fn compact(&mut self) {
        if let Some(current) = self.current_profile.clone() {
            if !self.profiles.contains_key(&current) {
                self.current_profile = self.profiles.keys().next().cloned();
            }
        }
    }
}

pub fn build_profile_record(
    config: &AuthConfig,
    profile_name: &str,
    refresh_token: &str,
    access_token: String,
    access_expires_in: u64,
    entitlements: Vec<String>,
    tier: Option<String>,
    email: Option<String>,
    beach_profile: Option<String>,
) -> Result<StoredProfile, AuthError> {
    let refresh = RefreshTokenRecord::write(profile_name, &config.gateway, refresh_token)?;
    let mut stored = StoredProfile {
        issuer: config.gateway.to_string(),
        audience: config.audience.clone(),
        tier,
        email,
        beach_profile,
        refresh,
        entitlements: entitlements.clone(),
        access_token: None,
        updated_at: OffsetDateTime::now_utc(),
    };
    stored.cache_access_token(access_token, access_expires_in, entitlements);
    Ok(stored)
}

pub fn access_token_is_valid(entry: &AccessTokenCache) -> bool {
    entry.expires_at > OffsetDateTime::now_utc() + Duration::seconds(30)
}

pub fn credentials_file_path() -> Result<PathBuf, AuthError> {
    CredentialsStore::path()
}

pub fn credentials_file_exists() -> Result<bool, AuthError> {
    let path = CredentialsStore::path()?;
    Ok(Path::new(&path).exists())
}

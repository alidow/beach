pub mod config;
pub mod credentials;
pub mod crypto;
pub mod error;
pub mod gate;
pub mod passphrase;

use crate::auth::config::AuthConfig;
use crate::auth::credentials::{
    CredentialsStore, FALLBACK_ENTITLEMENT, RefreshTokenRecord, StoredProfile, TURN_ENTITLEMENT,
    access_token_is_valid,
};
use crate::auth::error::AuthError;
use crate::auth::gate::{
    BeachGateClient, DeviceStartResponse, TokenResponse, TurnCredentialsResponse,
    access_token_expired,
};
use std::env;
use url::Url;

pub use config::AuthConfig as BeachAuthConfig;
pub use credentials::{
    AccessTokenCache, FALLBACK_ENTITLEMENT as FALLBACK_ENTITLEMENT_FLAG,
    TURN_ENTITLEMENT as TURN_ENTITLEMENT_FLAG,
};
pub use gate::DeviceStartResponse as AuthDeviceStartResponse;
pub use gate::TokenResponse as AuthTokenResponse;
pub use gate::TurnCredentialsResponse as AuthTurnCredentials;

pub const FRIENDLY_FALLBACK_MESSAGE: &str = "WebSocket fallback is only available to Beach Auth subscribers. Sign up at https://beach.sh and run `beach auth login` to unlock this transport.";

pub fn load_store() -> Result<CredentialsStore, AuthError> {
    CredentialsStore::load()
}

pub fn save_store(store: &CredentialsStore) -> Result<(), AuthError> {
    store.save()
}

fn env_truthy(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub fn manager_requires_access_token(base_url: &str) -> bool {
    if env_truthy("BEACH_MANAGER_REQUIRE_AUTH") {
        return true;
    }

    if let Ok(url) = Url::parse(base_url) {
        if let Some(host) = url.host_str() {
            let host = host.to_ascii_lowercase();
            if host.starts_with("private.")
                || host.contains(".private.")
                || host.contains("private-beach")
                || host.contains("pb-manager")
                || host.contains("beach-manager")
            {
                return true;
            }
        }
    }

    false
}

pub async fn maybe_access_token(
    profile_override: Option<&str>,
    refresh_if_needed: bool,
) -> Result<Option<String>, AuthError> {
    let profile_name = match active_profile_name(profile_override) {
        Ok(name) => name,
        Err(AuthError::NotLoggedIn) | Err(AuthError::ProfileNotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };

    let mut store = load_store()?;
    let profile = match store.profile(&profile_name).cloned() {
        Some(profile) => profile,
        None => return Ok(None),
    };

    if let Some(cache) = profile.access_token.as_ref() {
        if access_token_is_valid(cache) {
            return Ok(Some(cache.token.clone()));
        }
        if !refresh_if_needed {
            return Ok(None);
        }
    } else if !refresh_if_needed {
        return Ok(None);
    }

    let config = AuthConfig::from_env()?;
    let client = BeachGateClient::new(config.clone())?;
    let refresh_token = profile.refresh_token()?;
    let tokens = client.refresh_tokens(&refresh_token).await?;
    persist_profile_update(&profile_name, &tokens, &mut store, client.config())?;
    let entry = store
        .profile(&profile_name)
        .and_then(|profile| profile.access_token.as_ref())
        .ok_or_else(|| AuthError::Other("failed to cache refreshed access token".into()))?;
    Ok(Some(entry.token.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_private_hosts() {
        assert!(manager_requires_access_token("https://private.example.com"));
        assert!(manager_requires_access_token("https://pb-manager.test"));
        assert!(!manager_requires_access_token("https://api.beach.sh"));
    }

    #[test]
    fn env_override_forces_auth() {
        std::env::set_var("BEACH_MANAGER_REQUIRE_AUTH", "true");
        assert!(manager_requires_access_token("https://api.beach.sh"));
        std::env::remove_var("BEACH_MANAGER_REQUIRE_AUTH");
    }
}
pub fn apply_profile_environment(
    profile_override: Option<&str>,
) -> Result<Option<String>, AuthError> {
    let store = load_store()?;
    let resolved = store.ensure_profile_environment(profile_override);
    if let Some(profile) = &resolved {
        unsafe {
            env::set_var("BEACH_PROFILE", profile);
        }
    }
    Ok(resolved)
}

pub fn active_profile_name(profile_override: Option<&str>) -> Result<String, AuthError> {
    let store = load_store()?;
    if let Some(override_name) = profile_override {
        if store.profile(override_name).is_some() {
            return Ok(override_name.to_string());
        }
        return Err(AuthError::ProfileNotFound(override_name.to_string()));
    }

    if let Ok(env_profile) = env::var("BEACH_PROFILE") {
        let trimmed = env_profile.trim();
        if !trimmed.is_empty() {
            if store.profile(trimmed).is_some() {
                return Ok(trimmed.to_string());
            }
            return Err(AuthError::ProfileNotFound(trimmed.to_string()));
        }
    }

    if let Some(current) = store.current_profile.clone() {
        return Ok(current);
    }

    Err(AuthError::NotLoggedIn)
}

pub async fn refresh_profile_with_tokens(
    profile_name: &str,
    store: &mut CredentialsStore,
    client: &BeachGateClient,
) -> Result<StoredProfile, AuthError> {
    let profile = store
        .profile(profile_name)
        .cloned()
        .ok_or_else(|| AuthError::ProfileNotFound(profile_name.to_string()))?;

    let refresh_token = profile.refresh_token()?;
    let tokens = client.refresh_tokens(&refresh_token).await?;
    persist_profile_update(profile_name, &tokens, store, client.config())?;
    let updated = store
        .profile(profile_name)
        .cloned()
        .ok_or_else(|| AuthError::ProfileNotFound(profile_name.to_string()))?;
    Ok(updated)
}

pub fn persist_profile_update(
    profile_name: &str,
    tokens: &TokenResponse,
    store: &mut CredentialsStore,
    config: &AuthConfig,
) -> Result<(), AuthError> {
    let refresh = RefreshTokenRecord::write(profile_name, &config.gateway, &tokens.refresh_token)?;

    let mut profile = store
        .profile(profile_name)
        .cloned()
        .unwrap_or_else(|| StoredProfile {
            issuer: config.gateway.to_string(),
            audience: config.audience.clone(),
            tier: None,
            email: None,
            beach_profile: None,
            refresh: refresh.clone(),
            entitlements: Vec::new(),
            access_token: None,
            updated_at: time::OffsetDateTime::now_utc(),
        });

    profile.refresh = refresh;
    profile.entitlements = tokens.entitlements.clone();
    profile.tier = tokens.tier.clone();
    profile.email = tokens.email.clone();
    profile.beach_profile = tokens.profile.clone();
    profile.cache_access_token(
        tokens.access_token.clone(),
        tokens.access_token_expires_in,
        tokens.entitlements.clone(),
    );
    profile.issuer = config.gateway.to_string();
    profile.audience = config.audience.clone();

    store.upsert_profile(profile_name.to_string(), profile, false);
    store.save()?;
    Ok(())
}

pub async fn perform_device_login(
    _profile_name: &str,
    config: AuthConfig,
) -> Result<(DeviceStartResponse, BeachGateClient), AuthError> {
    let client = BeachGateClient::new(config)?;
    let start = client.start_device_flow().await?;
    Ok((start, client))
}

pub async fn complete_device_login(
    profile_name: &str,
    client: &BeachGateClient,
    device_code: &str,
) -> Result<TokenResponse, AuthError> {
    let tokens = client.finish_device_flow(device_code).await?;
    let mut store = load_store()?;
    persist_profile_update(profile_name, &tokens, &mut store, client.config())?;
    Ok(tokens)
}

pub fn ensure_profile_exists(profile_name: &str) -> Result<bool, AuthError> {
    let store = load_store()?;
    Ok(store.profile(profile_name).is_some())
}

pub fn remove_profile(profile_name: &str) -> Result<bool, AuthError> {
    let mut store = load_store()?;
    let removed = store.remove_profile(profile_name).is_some();
    store.save()?;
    Ok(removed)
}

pub fn set_current_profile(profile_name: Option<String>) -> Result<(), AuthError> {
    let mut store = load_store()?;
    store.set_current_profile(profile_name)?;
    store.save()?;
    Ok(())
}

pub fn status_for_profile(profile_name: Option<&str>) -> Result<Option<StoredProfile>, AuthError> {
    let store = load_store()?;
    let name = match profile_name {
        Some(name) => name.to_string(),
        None => store
            .current_profile
            .clone()
            .ok_or(AuthError::NotLoggedIn)?,
    };
    Ok(store.profile(&name).cloned())
}

pub async fn resolve_fallback_access_token(
    profile_override: Option<&str>,
) -> Result<String, AuthError> {
    let config = AuthConfig::from_env()?;
    let mut store = load_store()?;
    let profile_name = {
        if let Some(name) = profile_override {
            name.to_string()
        } else if let Ok(env_profile) = env::var("BEACH_PROFILE") {
            let trimmed = env_profile.trim();
            if !trimmed.is_empty() {
                trimmed.to_string()
            } else {
                store
                    .current_profile
                    .clone()
                    .ok_or(AuthError::NotLoggedIn)?
            }
        } else {
            store
                .current_profile
                .clone()
                .ok_or(AuthError::NotLoggedIn)?
        }
    };

    let profile = store
        .profile(&profile_name)
        .cloned()
        .ok_or_else(|| AuthError::ProfileNotFound(profile_name.clone()))?;

    if !profile.has_fallback_entitlement() {
        return Err(AuthError::FallbackNotEntitled);
    }

    if let Some(cache) = profile.access_token.as_ref() {
        if access_token_is_valid(cache)
            && cache
                .entitlements
                .iter()
                .any(|ent| ent == FALLBACK_ENTITLEMENT)
        {
            return Ok(cache.token.clone());
        }
        if !access_token_expired(cache.expires_at) {
            return Ok(cache.token.clone());
        }
    }

    let client = BeachGateClient::new(config.clone())?;
    let refresh_token = profile.refresh_token()?;
    let tokens = client.refresh_tokens(&refresh_token).await?;
    if !tokens
        .entitlements
        .iter()
        .any(|ent| ent == FALLBACK_ENTITLEMENT)
    {
        let mut store = load_store()?;
        if let Some(entry) = store.profile_mut(&profile_name) {
            entry.entitlements = tokens.entitlements.clone();
            entry.clear_access_token();
            store.save()?;
        }
        return Err(AuthError::FallbackNotEntitled);
    }

    persist_profile_update(&profile_name, &tokens, &mut store, client.config())?;
    let entry = store
        .profile(&profile_name)
        .and_then(|profile| profile.access_token.as_ref())
        .ok_or(AuthError::Other(
            "failed to cache refreshed access token".into(),
        ))?;
    Ok(entry.token.clone())
}

pub async fn resolve_turn_credentials(
    profile_override: Option<&str>,
) -> Result<TurnCredentialsResponse, AuthError> {
    let config = AuthConfig::from_env()?;
    let profile_name = active_profile_name(profile_override)?;
    let mut store = load_store()?;
    let profile = store
        .profile(&profile_name)
        .cloned()
        .ok_or_else(|| AuthError::ProfileNotFound(profile_name.clone()))?;

    if !profile.has_turn_entitlement() {
        return Err(AuthError::TurnNotEntitled);
    }

    let mut access_token: Option<String> = None;
    if let Some(cache) = profile.access_token.as_ref() {
        if access_token_is_valid(cache)
            && cache.entitlements.iter().any(|ent| ent == TURN_ENTITLEMENT)
        {
            access_token = Some(cache.token.clone());
        } else if !access_token_expired(cache.expires_at) {
            access_token = Some(cache.token.clone());
        }
    }

    let client = BeachGateClient::new(config.clone())?;

    if access_token.is_none() {
        let refresh_token = profile.refresh_token()?;
        let tokens = client.refresh_tokens(&refresh_token).await?;
        if !tokens
            .entitlements
            .iter()
            .any(|ent| ent == TURN_ENTITLEMENT)
        {
            if let Some(entry) = store.profile_mut(&profile_name) {
                entry.entitlements = tokens.entitlements.clone();
                entry.clear_access_token();
                store.save()?;
            }
            return Err(AuthError::TurnNotEntitled);
        }

        persist_profile_update(&profile_name, &tokens, &mut store, client.config())?;
        let updated = store
            .profile(&profile_name)
            .and_then(|profile| profile.access_token.as_ref())
            .ok_or_else(|| AuthError::Other("failed to cache refreshed access token".into()))?;
        access_token = Some(updated.token.clone());
    }

    let token = access_token.ok_or_else(|| AuthError::Other("missing access token".into()))?;
    let credentials = client.turn_credentials(&token).await?;
    Ok(credentials)
}

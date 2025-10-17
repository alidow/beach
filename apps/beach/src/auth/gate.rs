use crate::auth::config::AuthConfig;
use crate::auth::error::AuthError;
use reqwest::{Client, StatusCode};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone)]
pub struct BeachGateClient {
    client: Client,
    config: AuthConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenResponse {
    pub access_token: String,
    pub access_token_expires_in: u64,
    pub refresh_token: String,
    pub refresh_token_expires_in: u64,
    #[serde(default)]
    pub entitlements: Vec<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TurnCredentialsResponse {
    pub realm: String,
    pub ttl_seconds: u64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(rename = "iceServers")]
    pub ice_servers: Vec<TurnIceServer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurnIceServer {
    #[serde(deserialize_with = "deserialize_urls")]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
    #[serde(default, rename = "credentialType")]
    pub credential_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UrlsField {
    Single(String),
    Multiple(Vec<String>),
}

fn deserialize_urls<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let field = UrlsField::deserialize(deserializer)?;
    let mut urls: Vec<String> = match field {
        UrlsField::Single(value) => vec![value],
        UrlsField::Multiple(values) => values,
    };
    urls.retain(|url| !url.trim().is_empty());
    if urls.is_empty() {
        return Err(de::Error::custom("TURN server urls missing"));
    }
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_credentials_accept_single_url() {
        let json = r#"{
            "realm": "turn.example",
            "ttl_seconds": 120,
            "expires_at": null,
            "iceServers": [
                {
                    "urls": "turn:turn.example:3478",
                    "username": "alice",
                    "credential": "secret"
                }
            ]
        }"#;

        let creds: TurnCredentialsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(creds.ice_servers.len(), 1);
        assert_eq!(creds.ice_servers[0].urls, vec!["turn:turn.example:3478"]);
        assert_eq!(creds.ice_servers[0].username.as_deref(), Some("alice"));
        assert_eq!(creds.ice_servers[0].credential.as_deref(), Some("secret"));
    }

    #[test]
    fn turn_credentials_accept_multiple_urls() {
        let json = r#"{
            "realm": "turn.example",
            "ttl_seconds": 120,
            "expires_at": null,
            "iceServers": [
                {
                    "urls": ["turn:one.example:3478", "turn:two.example:3478"]
                }
            ]
        }"#;

        let creds: TurnCredentialsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(creds.ice_servers.len(), 1);
        assert_eq!(
            creds.ice_servers[0].urls,
            vec!["turn:one.example:3478", "turn:two.example:3478"]
        );
    }
}

impl BeachGateClient {
    pub fn new(config: AuthConfig) -> Result<Self, AuthError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| AuthError::Config(err.to_string()))?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    fn url(&self, path: &str) -> Result<Url, AuthError> {
        self.config
            .gateway
            .join(path)
            .map_err(|err| AuthError::Config(format!("invalid auth gateway path '{path}': {err}")))
    }

    pub async fn start_device_flow(&self) -> Result<DeviceStartResponse, AuthError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct RequestBody {
            scope: Option<String>,
            audience: Option<String>,
        }

        let url = self.url("device/start")?;
        let body = RequestBody {
            scope: self.config.scope.clone(),
            audience: self.config.audience.clone(),
        };

        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await?
            .error_for_status()
            .map_err(AuthError::from)?;

        Ok(response.json().await?)
    }

    pub async fn finish_device_flow(&self, device_code: &str) -> Result<TokenResponse, AuthError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct RequestBody<'a> {
            device_code: &'a str,
        }

        let url = self.url("device/finish")?;
        let response = self
            .client
            .post(url)
            .json(&RequestBody { device_code })
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let err_body: ErrorBody = serde_json::from_str(&text).unwrap_or(ErrorBody {
            error: None,
            detail: Some(text.clone()),
        });

        if status == StatusCode::BAD_REQUEST || status == StatusCode::BAD_GATEWAY {
            if let Some(detail) = err_body.detail.as_deref() {
                if detail.contains("authorization_pending") {
                    return Err(AuthError::AuthorizationPending);
                }
                if detail.contains("slow_down") {
                    return Err(AuthError::AuthorizationPending);
                }
                if detail.contains("access_denied") {
                    return Err(AuthError::AuthorizationDenied);
                }
            }
        }

        if status == StatusCode::UNAUTHORIZED {
            return Err(AuthError::AuthorizationDenied);
        }

        Err(AuthError::Gateway(format!(
            "device finish failed ({status}): {}",
            err_body
                .detail
                .or(err_body.error)
                .unwrap_or_else(|| "unknown error".into())
        )))
    }

    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct RefreshRequest<'a> {
            refresh_token: &'a str,
        }

        let url = self.url("token/refresh")?;
        let response = self
            .client
            .post(url)
            .json(&RefreshRequest { refresh_token })
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(AuthError::NotLoggedIn);
        }

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let err_body: ErrorBody = serde_json::from_str(&text).unwrap_or(ErrorBody {
            error: None,
            detail: Some(text.clone()),
        });

        Err(AuthError::Gateway(format!(
            "token refresh failed ({status}): {}",
            err_body
                .detail
                .or(err_body.error)
                .unwrap_or_else(|| "unknown error".into())
        )))
    }

    pub async fn turn_credentials(
        &self,
        access_token: &str,
    ) -> Result<TurnCredentialsResponse, AuthError> {
        let url = self.url("turn/credentials")?;
        let response = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let err_body: ErrorBody = serde_json::from_str(&text).unwrap_or(ErrorBody {
            error: None,
            detail: Some(text.clone()),
        });

        if status == StatusCode::FORBIDDEN {
            return Err(AuthError::TurnNotEntitled);
        }

        if status == StatusCode::UNAUTHORIZED {
            return Err(AuthError::NotLoggedIn);
        }

        if status == StatusCode::SERVICE_UNAVAILABLE {
            return Err(AuthError::Gateway(
                err_body
                    .detail
                    .or(err_body.error)
                    .unwrap_or_else(|| "turn credentials service unavailable".into()),
            ));
        }

        Err(AuthError::Gateway(format!(
            "turn credentials request failed ({status}): {}",
            err_body
                .detail
                .or(err_body.error)
                .unwrap_or_else(|| "unknown error".into())
        )))
    }
}

pub fn access_token_expired(expires_at: OffsetDateTime) -> bool {
    expires_at <= OffsetDateTime::now_utc() + time::Duration::seconds(15)
}

use crate::auth::config::AuthConfig;
use crate::auth::error::AuthError;
use reqwest::{Client, StatusCode};
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
}

pub fn access_token_expired(expires_at: OffsetDateTime) -> bool {
    expires_at <= OffsetDateTime::now_utc() + time::Duration::seconds(15)
}

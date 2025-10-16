//! Asynchronous client for Beach Manager APIs.
//!
//! This crate will be consumed by CLI tools, automation agents, and tests.
//! It should provide ergonomic wrappers around REST/MCP endpoints while
//! abstracting auth token handling and retry logic.

use reqwest::Client;
use serde::de::DeserializeOwned;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct ManagerClient {
    http: Client,
    base_url: String,
    token: String,
}

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected status: {status} body={body}")]
    UnexpectedStatus {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl ManagerClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
            token: token.into(),
        }
    }

    pub async fn list_sessions<T>(&self, private_beach_id: Uuid) -> Result<T, ManagerError>
    where
        T: DeserializeOwned,
    {
        let url = format!(
            "{}/private-beaches/{}/sessions",
            self.base_url, private_beach_id
        );
        let res = self.http.get(url).bearer_auth(&self.token).send().await?;

        if res.status().is_success() {
            Ok(res.json::<T>().await?)
        } else {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            Err(ManagerError::UnexpectedStatus { status, body })
        }
    }
}

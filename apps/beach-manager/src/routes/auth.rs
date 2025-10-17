use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};
use tracing::warn;

use crate::{auth::Claims, state::AppState};

use super::ApiError;

#[derive(Clone, Debug)]
pub struct AuthToken {
    raw: String,
    claims: Claims,
}

#[async_trait]
impl FromRequestParts<AppState> for AuthToken {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers).ok_or(ApiError::Unauthorized)?;
        let claims = state.auth_context().verify(&token).await.map_err(|err| {
            warn!(error = ?err, "token verification failed");
            ApiError::Unauthorized
        })?;
        Ok(AuthToken { raw: token, claims })
    }
}

impl AuthToken {
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn claims(&self) -> &Claims {
        &self.claims
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.claims.scopes().iter().any(|s| s == scope)
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(|token| token.to_owned())
}

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};
use tracing::warn;
use uuid::Uuid;

use crate::{auth::Claims, state::AppState};

use super::ApiError;

#[allow(dead_code)]
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

#[allow(dead_code)]
impl AuthToken {
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        fn matches_scope(candidate: &str, scope: &str) -> bool {
            candidate == "*"
                || candidate == scope
                || (candidate.ends_with(".*")
                    && scope.starts_with(&candidate[..candidate.len() - 2]))
        }

        if let Some(value) = &self.claims.scope {
            for item in value.split_whitespace() {
                if matches_scope(item, scope) {
                    return true;
                }
            }
        }
        if let Some(list) = &self.claims.scp {
            for candidate in list {
                if matches_scope(candidate, scope) {
                    return true;
                }
            }
        }
        false
    }

    pub fn account_id(&self) -> Option<&str> {
        self.claims.account_id.as_deref()
    }

    pub fn account_uuid(&self) -> Option<Uuid> {
        self.account_id().and_then(|id| Uuid::parse_str(id).ok())
    }

    #[allow(dead_code)]
    pub fn claims(&self) -> &Claims {
        &self.claims
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(|token| token.to_owned())
}

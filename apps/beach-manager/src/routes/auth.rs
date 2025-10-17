use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};

use super::ApiError;

#[derive(Clone, Debug)]
pub struct AuthToken(pub String);

#[async_trait]
impl<S> FromRequestParts<S> for AuthToken
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        extract_token(&parts.headers)
            .map(AuthToken)
            .ok_or(ApiError::Unauthorized)
    }
}

impl AuthToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(|token| token.to_owned())
}

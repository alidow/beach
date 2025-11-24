use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};
use tracing::warn;
use uuid::Uuid;

use crate::{auth::Claims, state::AppState};

use super::ApiError;

fn dev_insecure_enabled() -> bool {
    std::env::var("DEV_ALLOW_INSECURE_MANAGER_TOKEN").unwrap_or_default() == "1"
        && std::env::var("NODE_ENV").unwrap_or_default() != "production"
}

fn dev_insecure_token() -> String {
    std::env::var("DEV_MANAGER_INSECURE_TOKEN").unwrap_or_else(|_| "DEV-MANAGER-TOKEN".to_string())
}

fn dev_bypass_token() -> Option<String> {
    if dev_insecure_enabled() {
        return Some(dev_insecure_token());
    }
    None
}

fn dev_bypass_claims() -> Claims {
    Claims {
        sub: "dev-bypass".to_string(),
        iss: Some("beach-manager-dev".to_string()),
        aud: None,
        scope: Some("*".to_string()),
        scp: Some(vec!["*".to_string()]),
        account_id: Some("dev".to_string()),
        organization_id: None,
        private_beach_id: None,
        roles: vec!["dev".to_string()],
        exp: None,
    }
}

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
        let token = extract_token(&parts.headers)
            .or_else(|| extract_token_from_query(parts))
            .ok_or(ApiError::Unauthorized)?;

        if dev_insecure_enabled() {
            if let Some(dev_token) = dev_bypass_token() {
                if token == dev_token || dev_token.is_empty() {
                    return Ok(AuthToken {
                        raw: token,
                        claims: dev_bypass_claims(),
                    });
                }
            }
            // Accept any bearer token in dev insecure mode with wildcard scopes.
            return Ok(AuthToken {
                raw: token,
                claims: dev_bypass_claims(),
            });
        }

        let path = parts.uri.path().to_string();
        let strict = requires_strict_session_auth(&path);
        let ctx = state.auth_context();
        let result = if strict && !ctx.bypass_enabled() {
            ctx.verify_strict(&token).await
        } else {
            ctx.verify(&token).await
        };
        let claims = result.map_err(|err| {
            warn!(error = ?err, path = path, "token verification failed");
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

fn extract_token_from_query(parts: &Parts) -> Option<String> {
    let query = parts.uri.query()?;
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == "access_token" || k == "token" {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .to_string()
}

fn requires_strict_session_auth(path: &str) -> bool {
    if path.starts_with("/sessions") {
        return true;
    }
    if path.starts_with("/agents/") {
        return true;
    }
    if path.starts_with("/private-beaches/") && path.contains("/sessions") {
        return true;
    }
    false
}

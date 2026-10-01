//! Request authentication (HTTP side).

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use tracing::warn;

use super::error::ApiError;
use crate::app::AppState;
use crate::auth::Identity;
use crate::errors::{Error, ErrorCode};

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|t| !t.is_empty())
}

/// Authenticate from `Authorization: Bearer <token>`, falling back to `query_token`
/// (for WebSocket clients that cannot set headers). The token is never logged.
pub fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<Identity, ApiError> {
    let token = bearer(headers).or(query_token.filter(|t| !t.is_empty()));
    let result = match token {
        None => Err(Error::new(
            ErrorCode::Unauthenticated,
            "missing bearer token",
        )),
        Some(t) => state.auth.verify(t),
    };
    result.map_err(|e| {
        state.metrics.auth_failures_total.inc();
        warn!(reason = %e.message, "authentication failed");
        ApiError(e)
    })
}

/// Extractor yielding the verified identity of the caller.
pub struct Authed(pub Identity);

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        authenticate(state, &parts.headers, None).map(Authed)
    }
}

/// Constant-time byte comparison (for the optional metrics token).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    bearer(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_parsing() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, "Bearer abc.def".parse().unwrap());
        assert_eq!(bearer(&h), Some("abc.def"));
        h.insert(AUTHORIZATION, "bearer x".parse().unwrap());
        assert_eq!(bearer(&h), Some("x"));
        h.insert(AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(bearer(&h), None);
        h.insert(AUTHORIZATION, "Bearer ".parse().unwrap());
        assert_eq!(bearer(&h), None);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}

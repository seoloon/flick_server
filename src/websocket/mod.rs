//! WebSocket adapter: upgrade handler and per-connection loop.

mod connection;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::http::header::ORIGIN;
use axum::response::Response;
use serde::Deserialize;
use tracing::warn;

use crate::api::auth::authenticate_with;
use crate::api::error::ApiError;
use crate::app::AppState;
use crate::errors::{Error, ErrorCode};

#[derive(Deserialize)]
pub struct WsQuery {
    /// Fallback for clients that cannot set an `Authorization` header (browsers).
    access_token: Option<String>,
}

/// Browsers ignore CORS for WebSockets, so the `Origin` header is checked here.
/// Native clients send no `Origin` and are accepted; a present `Origin` must be allow-listed.
fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    match headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(origin) => {
            let origin = origin.trim_end_matches('/');
            allowed.iter().any(|a| a.eq_ignore_ascii_case(origin))
        }
    }
}

/// `GET /api/v1/rooms/{room_id}/ws`
pub async fn upgrade(
    State(state): State<AppState>,
    Path(room_id): Path<String>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // One snapshot for the whole upgrade: origin check and authentication agree.
    let server = state.server();
    if !origin_allowed(&headers, &server.cfg.http.allowed_origins) {
        warn!("websocket upgrade rejected: origin not allowed");
        return Err(Error::new(ErrorCode::Forbidden, "origin not allowed").into());
    }
    let identity = authenticate_with(&state, &server, &headers, query.access_token.as_deref())?;
    let sync = state.sync()?;
    // Fail with a regular HTTP error (404/403/409...) before upgrading when possible.
    sync.manager.can_attach(&room_id, &identity)?;
    let permit = sync
        .conn_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(ErrorCode::TooManyConnections, "too many connections"))?;

    let max = sync.ws.max_message_bytes;
    Ok(ws
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| connection::run(socket, state, sync, room_id, identity, permit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_policy() {
        let allowed = vec!["https://app.example".to_string()];
        let mut h = HeaderMap::new();
        assert!(
            origin_allowed(&h, &allowed),
            "native clients send no Origin"
        );
        h.insert(ORIGIN, "https://app.example".parse().unwrap());
        assert!(origin_allowed(&h, &allowed));
        h.insert(ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!origin_allowed(&h, &allowed));
        assert!(
            !origin_allowed(&h, &[]),
            "no allow-list: browsers are denied"
        );
    }
}

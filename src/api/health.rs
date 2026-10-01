//! Liveness, readiness and (optional) metrics.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::auth::{bearer_token, constant_time_eq};
use crate::app::AppState;

/// Liveness: the process is up and serving.
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

/// Readiness: able to authenticate users and accepting new work.
pub async fn ready(State(state): State<AppState>) -> Response {
    let ready = state.manager.is_accepting() && state.auth.has_keys();
    let (status, body) = if ready {
        (StatusCode::OK, json!({ "status": "ready" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "not_ready" }),
        )
    };
    (status, Json(body)).into_response()
}

/// Prometheus text metrics; 404 unless explicitly enabled, optionally token-protected.
pub async fn metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.cfg.http.metrics_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(expected) = &state.cfg.http.metrics_token {
        let ok = bearer_token(&headers)
            .is_some_and(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()));
        if !ok {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render_prometheus(),
    )
        .into_response()
}

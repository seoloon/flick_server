//! HTTP API (REST) and router assembly.

pub mod auth;
pub mod error;
pub mod health;
pub mod rooms;

use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method, header};
use axum::routing::{get, post};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::app::AppState;
use crate::websocket;

pub fn router(state: AppState) -> Router {
    let cors = cors_layer(&state.cfg.http.allowed_origins);
    let body_limit = state.cfg.http.max_body_bytes;

    let router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/metrics", get(health::metrics))
        .route("/api/v1/rooms", post(rooms::create_room))
        .route("/api/v1/rooms/{room_id}", get(rooms::get_room))
        .route("/api/v1/rooms/{room_id}/join", post(rooms::join_room))
        .route("/api/v1/rooms/{room_id}/leave", post(rooms::leave_room))
        .route("/api/v1/rooms/{room_id}/ws", get(websocket::upgrade))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state);
    match cors {
        Some(cors) => router.layer(cors),
        None => router,
    }
}

/// CORS is only enabled for explicitly configured origins; there is no wildcard.
/// Native clients do not send `Origin` and are unaffected either way.
fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let values: Vec<HeaderValue> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(values))
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(Duration::from_secs(600)),
    )
}

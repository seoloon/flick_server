//! HTTP API (REST) and router assembly.

pub mod admin;
pub mod auth;
pub mod downloads;
pub mod error;
pub mod health;
pub mod rooms;

use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method, header};
use axum::routing::{delete, get, post};
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
        .route("/admin/v1/overview", get(admin::overview))
        .route("/admin/v1/invite", get(admin::invite))
        .route("/admin/v1/rooms", get(admin::list_rooms))
        .route("/admin/v1/rooms/{room_id}", delete(admin::close_room))
        .route("/admin/v1/stats", get(admin::stats))
        .route("/api/v1/rooms", post(rooms::create_room))
        .route("/api/v1/rooms/{room_id}", get(rooms::get_room))
        .route("/api/v1/rooms/{room_id}/join", post(rooms::join_room))
        .route("/api/v1/rooms/{room_id}/leave", post(rooms::leave_room))
        .route("/api/v1/rooms/{room_id}/ws", get(websocket::upgrade))
        .route("/api/v1/downloads", post(downloads::create))
        .route(
            "/api/v1/downloads/{id}",
            get(downloads::status).delete(downloads::cancel),
        )
        .route("/api/v1/downloads/{id}/file", get(downloads::file))
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
            .allow_methods([Method::GET, Method::POST, Method::DELETE])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::RANGE,
                header::IF_RANGE,
            ])
            .expose_headers([
                header::CONTENT_RANGE,
                header::ETAG,
                header::ACCEPT_RANGES,
                header::CONTENT_LENGTH,
                header::RETRY_AFTER,
            ])
            .max_age(Duration::from_secs(600)),
    )
}

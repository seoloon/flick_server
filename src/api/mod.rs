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
    let cors = cors_layer(&state);
    let body_limit = state.boot.http.max_body_bytes;

    // Every admin response (success, 204 and errors) is uncacheable.
    let admin = Router::new()
        .route("/admin/v1/overview", get(admin::overview))
        .route("/admin/v1/invite", get(admin::invite))
        .route("/admin/v1/rooms", get(admin::list_rooms))
        .route("/admin/v1/rooms/{room_id}", delete(admin::close_room))
        .route("/admin/v1/stats", get(admin::stats))
        .route("/admin/v1/dd/overview", get(admin::dd_overview))
        .route("/admin/v1/dd/active", get(admin::dd_active))
        .route("/admin/v1/dd/history", get(admin::dd_history))
        .route("/admin/v1/dd/stats", get(admin::dd_stats))
        .route("/admin/v1/dd/{id}", delete(admin::dd_cancel))
        .route("/admin/v1/modules", get(admin::modules))
        .route(
            "/admin/v1/modules/{id}/{action}",
            post(admin::module_action),
        )
        .route(
            "/admin/v1/settings/{scope}",
            get(admin::get_settings).put(admin::put_settings),
        )
        .route(
            "/admin/v1/settings/server/reload",
            post(admin::reload_server),
        )
        .route("/admin/v1/logs", get(admin::logs))
        .layer(axum::middleware::map_response(admin::no_store_layer));

    Router::new()
        .merge(admin)
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/metrics", get(health::metrics))
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
        .with_state(state)
        .layer(cors)
}

/// CORS follows the origins of the server runtime in force, read on every request; there is
/// no wildcard. Native clients do not send `Origin` and are unaffected either way.
fn cors_layer(state: &AppState) -> CorsLayer {
    let state = state.clone();
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            state
                .server()
                .cfg
                .http
                .allowed_origins
                .iter()
                .any(|o| o.as_bytes().eq_ignore_ascii_case(origin.as_bytes()))
        }))
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
        .max_age(Duration::from_secs(600))
}

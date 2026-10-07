//! Operator API for the web panel: `/admin/v1/*`.
//!
//! Disabled (404) unless `FLICKSYNC_ADMIN_TOKEN` is set; then every call needs
//! `Authorization: Bearer <token>`, compared in constant time. It exposes the invitation
//! (which contains the signing key), so responses are never cacheable and the token must stay
//! server-side: the panel's server calls this API, browsers never see the token.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;
use tracing::warn;

use super::auth::{bearer_token, constant_time_eq};
use super::error::ApiError;
use crate::app::AppState;
use crate::auth::parse_key_entry;
use crate::dd::{DdState, now_ms as dd_now_ms};
use crate::errors::{Error, ErrorCode};
use crate::invite;
use crate::room::AdminRoomView;
use crate::room::manager::share_code;

/// Extractor: succeeds only for a valid admin token.
pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(expected) = &state.cfg.http.admin_token else {
            return Err(StatusCode::NOT_FOUND.into_response());
        };
        let ok = bearer_token(&parts.headers)
            .is_some_and(|t| constant_time_eq(t.as_bytes(), expected.as_bytes()));
        if ok {
            Ok(AdminAuth)
        } else {
            state.metrics.auth_failures_total.inc();
            warn!("admin API: invalid or missing token");
            Err(ApiError(Error::new(
                ErrorCode::Unauthenticated,
                "invalid admin token",
            ))
            .into_response())
        }
    }
}

fn no_store<T: Serialize>(status: StatusCode, body: T) -> Response {
    let mut r = (status, Json(body)).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `GET /admin/v1/overview`
pub async fn overview(_: AdminAuth, State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    no_store(
        StatusCode::OK,
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "now": now_ms(),
            "uptime_secs": state.started_at.elapsed().as_secs(),
            "accepting": state.manager.is_accepting(),
            "ready": state.manager.is_accepting() && state.auth.has_keys(),
            "rooms": state.manager.room_count(),
            "max_rooms": state.cfg.manager.max_rooms,
            "participants": m.participants_active.get(),
            "connections": m.ws_connections.get(),
            "max_connections": state.cfg.ws.max_connections,
            "rtt_avg_ms": m.rtt_avg_us.get() as f64 / 1000.0,
        }),
    )
}

/// `GET /admin/v1/invite`: the one-link invitation (contains the signing key).
pub async fn invite(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let inv = invite::invitation(&state.cfg)
        .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?;
    let (endpoint, guessed) = invite::endpoint(&state.cfg);
    let url = inv.to_url();
    let (kid, server_id) = parse_key_entry(&inv.key)
        .map(|(k, s, _)| (k.to_owned(), s.to_owned()))
        .unwrap_or_default();
    Ok(no_store(
        StatusCode::OK,
        json!({
            "url": url,
            "address": endpoint.http_base(),
            "tls": endpoint.tls,
            "address_guessed": guessed,
            "key_source": if state.cfg.keys_configured { "environment" } else { "file" },
            "key_count": state.cfg.auth.keys.len(),
            "kid": kid,
            "server_id": server_id,
            "qr": invite::qr_modules(&url),
        }),
    ))
}

#[derive(Serialize)]
struct AdminRoom {
    #[serde(flatten)]
    room: AdminRoomView,
    share_code: String,
}

/// `GET /admin/v1/rooms`
pub async fn list_rooms(_: AdminAuth, State(state): State<AppState>) -> Response {
    let rooms: Vec<AdminRoom> = state
        .manager
        .admin_rooms()
        .into_iter()
        .map(|room| AdminRoom {
            share_code: share_code(&room.room_id),
            room,
        })
        .collect();
    no_store(StatusCode::OK, json!({ "now": now_ms(), "rooms": rooms }))
}

/// `DELETE /admin/v1/rooms/{room_id}`: force-close a (frozen) room.
pub async fn close_room(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(room_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.manager.admin_close_room(&room_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /admin/v1/stats`: counters, drift distribution and a rolling history (1 h, 10 s steps).
pub async fn stats(_: AdminAuth, State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let d = &state.cfg.manager.room.drift;
    no_store(
        StatusCode::OK,
        json!({
            "now": now_ms(),
            "uptime_secs": state.started_at.elapsed().as_secs(),
            "rtt_avg_ms": m.rtt_avg_us.get() as f64 / 1000.0,
            "totals": {
                "rooms_created": m.rooms_created_total.get(),
                "rooms_destroyed": m.rooms_destroyed_total.get(),
                "messages_in": m.messages_in_total.get(),
                "malformed_messages": m.malformed_messages_total.get(),
                "rate_limited": m.rate_limited_total.get(),
                "auth_failures": m.auth_failures_total.get(),
                "sync_reports": m.sync_reports_total.get(),
                "sync_corrections": m.sync_corrections_total.get(),
                "sync_seeks": m.sync_seeks_total.get(),
            },
            "drift": {
                "thresholds_ms": { "ignore": d.ignore_ms, "soft": d.soft_ms, "hard": d.hard_ms },
                "buckets": m.drift_buckets.iter().map(|c| c.get()).collect::<Vec<_>>(),
            },
            "history_interval_secs": crate::metrics::History::INTERVAL_MS / 1000,
            "history": m.history.snapshot(),
        }),
    )
}

/// The FlickDD state, or 404 when FlickDD is disabled.
fn dd_state(state: &AppState) -> Result<&std::sync::Arc<DdState>, ApiError> {
    state
        .dd
        .as_ref()
        .ok_or_else(|| Error::new(ErrorCode::DownloadNotFound, "FlickDD is disabled").into())
}

/// `GET /admin/v1/dd/overview`: configuration limits and headline numbers.
pub async fn dd_overview(
    _: AdminAuth,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let dd = dd_state(&state)?;
    let c = &dd.cfg;
    let snap = dd.stats.snapshot(dd_now_ms());
    Ok(no_store(
        StatusCode::OK,
        json!({
            "enabled": c.enabled,
            "backends": { "jellyfin": c.jellyfin.is_some(), "plex": c.plex.is_some() },
            "limits": {
                "max_parallel": c.max_parallel,
                "max_global": c.max_global,
                "rate_bps": c.rate_bps,
                "chunk_bytes": c.chunk_bytes,
                "max_range_bytes": c.max_range_bytes,
                "grant_ttl_secs": c.grant_ttl_ms / 1000,
            },
            "active": dd.grants.count(),
            "totals": snap.totals,
        }),
    ))
}

/// `GET /admin/v1/dd/active`
pub async fn dd_active(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let dd = dd_state(&state)?;
    let now = dd_now_ms();
    Ok(no_store(
        StatusCode::OK,
        json!({ "now": now, "downloads": dd.grants.active_views(now) }),
    ))
}

/// `GET /admin/v1/dd/history`: recently finished downloads, newest first.
pub async fn dd_history(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let dd = dd_state(&state)?;
    Ok(no_store(
        StatusCode::OK,
        json!({ "now": dd_now_ms(), "downloads": dd.stats.history() }),
    ))
}

/// `GET /admin/v1/dd/stats`
pub async fn dd_stats(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let dd = dd_state(&state)?;
    let now = dd_now_ms();
    let mut body = serde_json::to_value(dd.stats.snapshot(now))
        .map_err(|e| Error::new(ErrorCode::Internal, e.to_string()))?;
    body["now"] = json!(now);
    Ok(no_store(StatusCode::OK, body))
}

/// `DELETE /admin/v1/dd/{id}`: stop a download and free its slot.
pub async fn dd_cancel(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let dd = dd_state(&state)?;
    if dd.grants.cancel(&id, true, dd_now_ms()) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error::new(ErrorCode::DownloadNotFound, "download not found").into())
    }
}

//! Operator API for the web panel: `/admin/v1/*`.
//!
//! Disabled (404) unless `PANEL_PASSWORD` (10+ characters) or the deprecated
//! `FLICKSYNC_ADMIN_TOKEN` is set; then every call needs
//! `Authorization: Bearer <token>`, compared in constant time. It exposes the invitation
//! (which contains the signing key), so responses are never cacheable and the token must stay
//! server-side: the panel's server calls this API, browsers never see the token.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use super::auth::bearer_token;
use super::error::ApiError;
use crate::app::AppState;
use crate::auth::parse_key_entry;
use crate::dd::{DdState, now_ms as dd_now_ms};
use crate::errors::{Error, ErrorCode};
use crate::invite;
use crate::logs::{LogEntry, LogQuery};
use crate::modules::ModuleId;
use crate::room::AdminRoomView;
use crate::room::manager::share_code;
use crate::settings::SettingsError;
use crate::settings::store::{self, Scope, StoreError};

/// Extractor: succeeds only for a valid admin token.
pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if !state.admin.is_enabled() {
            return Err(StatusCode::NOT_FOUND.into_response());
        }
        let message = match bearer_token(&parts.headers) {
            Some(t) if state.admin.accepts(t) => return Ok(AdminAuth),
            Some(_) => {
                "The admin token was not accepted. It must be derived from the server's PANEL_PASSWORD (or equal the deprecated FLICKSYNC_ADMIN_TOKEN): check that the panel and the server use the same password."
            }
            None => {
                "The request carries no admin token. Send Authorization: Bearer <token>, with the token derived from PANEL_PASSWORD."
            }
        };
        state.metrics.auth_failures_total.inc();
        warn!("admin API: invalid or missing token");
        Err(ApiError(Error::new(ErrorCode::Unauthenticated, message)).into_response())
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
    let sync = state.sync_opt();
    no_store(
        StatusCode::OK,
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "now": now_ms(),
            "uptime_secs": state.started_at.elapsed().as_secs(),
            "running": sync.is_some(),
            "accepting": sync.as_ref().is_some_and(|s| s.manager.is_accepting()),
            "ready": state.server().auth.has_keys(),
            "rooms": sync.as_ref().map_or(0, |s| s.manager.room_count()),
            "max_rooms": sync.as_ref().map_or(0, |s| s.manager.config().max_rooms),
            "participants": m.participants_active.get(),
            "connections": m.ws_connections.get(),
            "max_connections": sync.as_ref().map_or(0, |s| s.ws.max_connections),
            "rtt_avg_ms": m.rtt_avg_us.get() as f64 / 1000.0,
        }),
    )
}

/// `GET /admin/v1/invite`: the one-link invitation (contains the signing key).
pub async fn invite(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let server = state.server();
    let inv = invite::invitation(&server.cfg).map_err(|e| {
        warn!(error = %e, "admin API: cannot build the invitation");
        Error::new(
            ErrorCode::Internal,
            "The invitation could not be built from the signing key in force. Check FLICKSYNC_AUTH_KEYS (or the key file), then reload the server settings.",
        )
    })?;
    let (endpoint, guessed) = invite::endpoint(&server.cfg);
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
            "key_source": if server.cfg.keys_configured { "environment" } else { "file" },
            "key_count": server.cfg.auth.keys.len(),
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

/// Why `id` is not running, and what to do about it.
fn not_running(state: &AppState, id: ModuleId, label: &str) -> String {
    let status = state.module_status(id);
    match status.message {
        Some(reason) if status.state == "failed" => {
            format!("{label} is enabled but failed to start. {reason}")
        }
        _ => format!(
            "{label} is stopped. Start it with POST /admin/v1/modules/{}/start.",
            id.id()
        ),
    }
}

/// The running FlickSync, or `MODULE_DISABLED` saying why.
fn admin_sync(state: &AppState) -> Result<Arc<crate::app::SyncRuntime>, ApiError> {
    state.sync_opt().ok_or_else(|| {
        Error::new(
            ErrorCode::ModuleDisabled,
            not_running(state, ModuleId::FlickSync, "FlickSync"),
        )
        .into()
    })
}

/// `GET /admin/v1/rooms`
pub async fn list_rooms(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let sync = admin_sync(&state)?;
    let rooms: Vec<AdminRoom> = sync
        .manager
        .admin_rooms()
        .into_iter()
        .map(|room| AdminRoom {
            share_code: share_code(&room.room_id),
            room,
        })
        .collect();
    Ok(no_store(
        StatusCode::OK,
        json!({ "now": now_ms(), "rooms": rooms }),
    ))
}

/// `DELETE /admin/v1/rooms/{room_id}`: force-close a (frozen) room.
pub async fn close_room(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(room_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    admin_sync(&state)?
        .manager
        .admin_close_room(&room_id)
        .map_err(|e| match e.code {
            ErrorCode::RoomNotFound => Error::new(
                ErrorCode::RoomNotFound,
                "No live room has this id: it may have closed already.",
            ),
            _ => e,
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /admin/v1/stats`: counters, drift distribution and a rolling history (1 h, 10 s steps).
pub async fn stats(_: AdminAuth, State(state): State<AppState>) -> Result<Response, ApiError> {
    let sync = admin_sync(&state)?;
    let m = &state.metrics;
    let d = &sync.manager.config().room.drift;
    Ok(no_store(
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
    ))
}

/// The FlickDD state, or 404 `DOWNLOAD_NOT_FOUND` (kept for the panel) saying why it is not
/// running.
fn dd_state(state: &AppState) -> Result<Arc<DdState>, ApiError> {
    state.dd().ok_or_else(|| {
        Error::new(
            ErrorCode::DownloadNotFound,
            not_running(state, ModuleId::FlickDd, "FlickDD"),
        )
        .into()
    })
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
    let mut body = serde_json::to_value(dd.stats.snapshot(now)).map_err(|e| {
        warn!(error = %e, "admin API: cannot encode the FlickDD stats");
        Error::new(
            ErrorCode::Internal,
            "The FlickDD statistics could not be encoded. Try again; the server logs have the details.",
        )
    })?;
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
        Err(Error::new(
            ErrorCode::DownloadNotFound,
            "No download with this id is in progress: it may have finished, expired or been cancelled already.",
        )
        .into())
    }
}

/// Layer applied to every admin route: `Cache-Control: no-store` on all responses,
/// including 204s and error bodies.
pub async fn no_store_layer(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError(Error::new(ErrorCode::InvalidPayload, message))
}

/// Comma-separated ids, for "valid ones are ..." hints.
fn list(ids: impl IntoIterator<Item = &'static str>) -> String {
    ids.into_iter().collect::<Vec<_>>().join(", ")
}

fn unknown_scope(scope: &str) -> ApiError {
    ApiError(Error::new(
        ErrorCode::UnknownScope,
        format!(
            "Unknown settings scope '{}'. Valid scopes: {}.",
            crate::settings::shorten(scope),
            list(Scope::ALL.map(Scope::id))
        ),
    ))
}

/// `what` is the change being saved ("settings", "module switch") for the message.
fn settings_error(e: SettingsError, what: &str) -> ApiError {
    let (code, message) = match e {
        SettingsError::Store(inner) => {
            warn!(error = %inner, "could not save the settings");
            let cause = match &inner {
                StoreError::Write { source, .. } => source.kind().to_string(),
                _ => "unexpected error".to_owned(),
            };
            (
                ErrorCode::SettingsWriteFailed,
                format!(
                    "The {what} could not be saved: {} in the data directory cannot be written ({cause}). Nothing was changed; check that the data volume is writable by the server, then try again.",
                    store::FILE_NAME
                ),
            )
        }
        e @ SettingsError::UnknownField { .. } => (ErrorCode::UnknownSetting, e.to_string()),
        e @ (SettingsError::TooLong(_) | SettingsError::Invalid(_)) => (
            ErrorCode::SettingsInvalid,
            format!("{e} Nothing was saved."),
        ),
    };
    ApiError(Error::new(code, message))
}

/// `GET /admin/v1/modules`
pub async fn modules(_: AdminAuth, State(state): State<AppState>) -> Response {
    let list: Vec<_> = ModuleId::ALL
        .into_iter()
        .map(|id| state.module_status(id))
        .collect();
    no_store(StatusCode::OK, json!({ "modules": list }))
}

/// `POST /admin/v1/modules/{id}/{start|stop|reload}`
pub async fn module_action(
    _: AdminAuth,
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    const ACTIONS: [&str; 3] = ["start", "stop", "reload"];
    let Some(id) = ModuleId::from_id(&id) else {
        return Err(ApiError(Error::new(
            ErrorCode::UnknownModule,
            format!(
                "Unknown module '{}'. Valid modules: {}.",
                crate::settings::shorten(&id),
                list(ModuleId::ALL.map(ModuleId::id))
            ),
        )));
    };
    let switch = |e| settings_error(e, "module switch");
    let status = match action.as_str() {
        "start" => state.start_module(id).map_err(switch)?,
        "stop" => state.stop_module(id).map_err(switch)?,
        "reload" => state.reload_module(id),
        _ => {
            return Err(ApiError(Error::new(
                ErrorCode::UnknownAction,
                format!(
                    "Unknown action '{}' for module {}. Valid actions: {}.",
                    crate::settings::shorten(&action),
                    id.id(),
                    list(ACTIONS)
                ),
            )));
        }
    };
    Ok(no_store(StatusCode::OK, status))
}

/// `GET /admin/v1/settings/{scope}`
pub async fn get_settings(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(scope): Path<String>,
) -> Result<Response, ApiError> {
    let scope = Scope::from_id(&scope).ok_or_else(|| unknown_scope(&scope))?;
    Ok(no_store(StatusCode::OK, state.settings.view(scope)))
}

/// The refusal of a body that is not `{"values": {...}}`. Serde's own text is not echoed: it
/// can quote the request, and the request can hold a secret.
fn body_error(e: &JsonRejection) -> ApiError {
    const SHAPE: &str = r#"{"values": {"NAME": value}}"#;
    bad_request(match e {
        JsonRejection::MissingJsonContentType(_) => {
            format!(
                "The request needs the header Content-Type: application/json, with a body of the form {SHAPE}."
            )
        }
        JsonRejection::JsonSyntaxError(_) => {
            format!("The body is not valid JSON. Expected the form {SHAPE}.")
        }
        _ => format!(
            "The body must have the form {SHAPE}: an object named values whose keys are setting names."
        ),
    })
}

#[derive(Deserialize)]
pub struct PutSettings {
    values: BTreeMap<String, serde_json::Value>,
}

/// `PUT /admin/v1/settings/{scope}`: a partial update. Strings, numbers and booleans set a
/// value, `null` removes it, an omitted name is left alone.
pub async fn put_settings(
    _: AdminAuth,
    State(state): State<AppState>,
    Path(scope): Path<String>,
    body: Result<Json<PutSettings>, JsonRejection>,
) -> Result<Response, ApiError> {
    let scope = Scope::from_id(&scope).ok_or_else(|| unknown_scope(&scope))?;
    let Json(body) = body.map_err(|e| body_error(&e))?;
    let mut patch = BTreeMap::new();
    for (name, value) in body.values {
        let value = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => {
                return Err(bad_request(format!(
                    "{} has a value of the wrong type (an array or an object). Send a string, a number, a boolean, or null to remove the stored value.",
                    crate::settings::shorten(&name)
                )));
            }
        };
        patch.insert(name, value);
    }
    let view = state
        .settings
        .put(scope, patch)
        .map_err(|e| settings_error(e, "settings"))?;
    Ok(no_store(StatusCode::OK, view))
}

/// `POST /admin/v1/settings/server/reload`
pub async fn reload_server(
    _: AdminAuth,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    state.reload_server().map_err(|e| {
        warn!(error = %e, "admin API: the server settings could not be reloaded");
        ApiError(e.admin_error())
    })?;
    Ok(no_store(StatusCode::OK, json!({ "reloaded": true })))
}

/// `GET /admin/v1/logs?after=<seq>&level=<min>&limit=<n>`: the latest log lines kept in memory,
/// oldest first, after a cursor. Lost on restart; `boot` tells one run from the next.
pub async fn logs(
    _: AdminAuth,
    State(state): State<AppState>,
    query: Result<Query<Vec<(String, String)>>, QueryRejection>,
) -> Result<Response, ApiError> {
    let invalid = |message: String| ApiError(Error::new(ErrorCode::InvalidQuery, message));
    let Query(pairs) = query.map_err(|_| {
        invalid(
            "The query string could not be read. Expected after=<seq>, level=<error|warn|info|debug|trace> and limit=<1-1000>, each at most once."
                .to_owned(),
        )
    })?;
    let q = LogQuery::from_pairs(&pairs).map_err(invalid)?;
    let page = state.logs.query(&q);
    // Built after `query` returned, outside the buffer's lock.
    let entries: Vec<&LogEntry> = page.entries.iter().map(|e| e.as_ref()).collect();
    Ok(no_store(
        StatusCode::OK,
        json!({
            "boot": state.logs.boot(),
            "capacity": state.logs.capacity(),
            "log_level": state.server().cfg.log_level,
            "entries": entries,
            "next": page.next,
            "more": page.more,
            "dropped": page.dropped,
        }),
    ))
}

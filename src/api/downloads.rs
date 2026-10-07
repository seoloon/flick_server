//! FlickDD client routes: create a download grant, read its status, stream its bytes, cancel.
//!
//! The download token (`Authorization: Bearer` or `?token=`) is a bearer secret: neither it
//! nor the query string carrying it ever reaches a log line or an error message.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

use super::auth::{authenticate, bearer_token};
use super::error::ApiError;
use crate::app::AppState;
use crate::auth::PERM_DOWNLOAD;
use crate::dd::backend::{BackendError, BackendKind, valid_item_id};
use crate::dd::grants::{GrantError, GrantView, NewGrant};
use crate::dd::pump::{self, PumpParams};
use crate::dd::ranges::{self, Resolved};
use crate::dd::{DdState, now_ms};
use crate::errors::{Error, ErrorCode};

const MAX_TITLE_CHARS: usize = 200;
/// Pause before the single inline retry of the upstream open.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Error of these routes: the usual `{error: {code, message}}` body, plus `Retry-After` when
/// known and an optional status override (503 for a backend that is not configured).
pub struct DdError {
    error: Error,
    retry_after: Option<u64>,
    status: Option<StatusCode>,
}

impl From<Error> for DdError {
    fn from(error: Error) -> Self {
        Self {
            error,
            retry_after: None,
            status: None,
        }
    }
}

impl From<ApiError> for DdError {
    fn from(e: ApiError) -> Self {
        e.0.into()
    }
}

impl From<GrantError> for DdError {
    fn from(e: GrantError) -> Self {
        Self {
            error: e.error,
            retry_after: e.retry_after_secs,
            status: None,
        }
    }
}

impl IntoResponse for DdError {
    fn into_response(self) -> Response {
        let mut resp = ApiError(self.error).into_response();
        if let Some(status) = self.status {
            *resp.status_mut() = status;
        }
        if let Some(secs) = self.retry_after {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        resp
    }
}

fn enabled(state: &AppState) -> Result<Arc<DdState>, DdError> {
    state
        .dd()
        .ok_or_else(|| Error::new(ErrorCode::DownloadNotFound, "FlickDD is disabled").into())
}

fn invalid(message: &str) -> DdError {
    Error::new(ErrorCode::InvalidPayload, message).into()
}

fn not_configured() -> DdError {
    DdError {
        status: Some(StatusCode::SERVICE_UNAVAILABLE),
        ..Error::new(
            ErrorCode::BackendUnavailable,
            "this media backend is not configured",
        )
        .into()
    }
}

/// Client-facing mapping of a backend failure. The inner detail of `Unavailable` is logged
/// for the operator (it never carries secrets) but never returned.
fn backend_error(kind: BackendKind, e: BackendError) -> DdError {
    match e {
        BackendError::NotFound => Error::new(ErrorCode::DownloadNotFound, "item not found").into(),
        BackendError::SourceChanged => Error::new(
            ErrorCode::SourceChanged,
            "the file changed on the media server; create a new download",
        )
        .into(),
        BackendError::NotConfigured => not_configured(),
        BackendError::Unavailable(detail) => {
            warn!(backend = kind.as_str(), error = %detail, "FlickDD backend unavailable");
            Error::new(
                ErrorCode::BackendUnavailable,
                "the media backend is unavailable",
            )
            .into()
        }
    }
}

// --- POST /api/v1/downloads ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ItemKind {
    Movie,
    Episode,
}

impl ItemKind {
    fn as_str(self) -> &'static str {
        match self {
            ItemKind::Movie => "movie",
            ItemKind::Episode => "episode",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    backend: BackendKind,
    item_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    kind: Option<ItemKind>,
}

/// Trimmed, at most [`MAX_TITLE_CHARS`] characters, `None` when empty.
fn clean_title(title: Option<String>) -> Option<String> {
    let t: String = title?.trim().chars().take(MAX_TITLE_CHARS).collect();
    let t = t.trim_end();
    (!t.is_empty()).then(|| t.to_owned())
}

/// `POST /api/v1/downloads`: resolve the item on its backend and open a grant.
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, DdError> {
    let dd = enabled(&state)?;
    let identity = authenticate(&state, &headers, None)?;
    identity.require(PERM_DOWNLOAD)?;
    // Same key as the room rate limiter: user ids are only unique per Flick server.
    let user_key = format!("{}/{}", identity.server_id, identity.user_id);
    // Every attempt counts, refused ones included: a client looping on create is stopped.
    dd.admit_create(&user_key, dd.now_mono_ms())?;
    let req: CreateRequest = serde_json::from_slice(&body).map_err(|_| {
        invalid("expected {backend: jellyfin|plex, item_id, title?, kind?: movie|episode} as JSON")
    })?;
    if !valid_item_id(&req.item_id) {
        return Err(invalid("item_id must match [A-Za-z0-9_-]{1,64}"));
    }
    if !dd.backends.has(req.backend) {
        return Err(not_configured());
    }
    // No free slot: refuse before asking the backend anything. `Grants::create` checks
    // again after the resolve, under the same lock as the slot it takes.
    dd.grants.check_slots(&user_key)?;
    let file = dd
        .backends
        .resolve(req.backend, &req.item_id)
        .await
        .map_err(|e| backend_error(req.backend, e))?;
    let (size, filename, mime, etag) = (
        file.size,
        file.filename.clone(),
        file.mime.clone(),
        file.etag.clone(),
    );
    let now = now_ms();
    let created = dd.grants.create(
        NewGrant {
            user_id: user_key,
            user_name: identity.display_name,
            backend: req.backend,
            item_id: req.item_id,
            title: clean_title(req.title),
            kind: req.kind.map(|k| k.as_str().to_owned()),
            file,
        },
        now,
    )?;
    let cfg = &dd.cfg;
    let body = json!({
        "url": format!("/api/v1/downloads/{}/file", created.id),
        "download_id": created.id,
        "token": created.token,
        "size": size,
        "filename": filename,
        "mime": mime,
        "etag": etag,
        "chunk_bytes": cfg.chunk_bytes,
        "max_range_bytes": cfg.max_range_bytes,
        "rate_limit_bps": cfg.rate_bps,
        // A fresh grant: idle TTL and absolute age both start now.
        "expires_at": now.saturating_add(cfg.grant_ttl_ms.min(cfg.grant_max_age_ms)),
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

// --- Token routes -------------------------------------------------------------------------

/// `?token=` for clients (OS background downloaders) that cannot set headers.
#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

type TokenParam = Result<Query<TokenQuery>, QueryRejection>;

/// The download token: `Authorization: Bearer` wins over `?token=`. Missing gives 401.
fn download_token<'a>(headers: &'a HeaderMap, query: &'a TokenParam) -> Result<&'a str, DdError> {
    bearer_token(headers)
        .or_else(|| {
            let q = query.as_ref().ok()?;
            q.token.as_deref().map(str::trim).filter(|t| !t.is_empty())
        })
        .ok_or_else(|| Error::new(ErrorCode::Unauthenticated, "missing download token").into())
}

/// `GET /api/v1/downloads/{id}`: size, bytes covered so far, validator and expiry.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: TokenParam,
) -> Result<Json<Value>, DdError> {
    let dd = enabled(&state)?;
    let g = dd
        .grants
        .authorize(&id, download_token(&headers, &query)?, now_ms())?;
    Ok(Json(json!({
        "download_id": g.id,
        "size": g.size,
        "covered": g.covered,
        "etag": g.etag,
        "expires_at": g.expires_at,
    })))
}

/// `DELETE /api/v1/downloads/{id}`: cancel, close its stream and free the slot.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: TokenParam,
) -> Result<StatusCode, DdError> {
    let dd = enabled(&state)?;
    dd.grants
        .authorize(&id, download_token(&headers, &query)?, now_ms())?;
    dd.grants.cancel(&id, false, now_ms());
    Ok(StatusCode::NO_CONTENT)
}

/// The `Range` to honour: none when absent, or when `If-Range` does not match the grant's
/// (strong) ETag, in which case the whole file is served (RFC 9110 13.1.5).
fn requested_range<'a>(headers: &'a HeaderMap, etag: &str) -> Option<&'a str> {
    let range = headers.get(header::RANGE)?.to_str().ok()?;
    match headers.get(header::IF_RANGE) {
        None => Some(range),
        Some(v) => (v.to_str().ok().map(str::trim) == Some(etag)).then_some(range),
    }
}

fn unsatisfiable(size: u64) -> Response {
    let mut resp = DdError::from(Error::new(
        ErrorCode::RangeNotSatisfiable,
        "the requested range is not satisfiable",
    ))
    .into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
        resp.headers_mut().insert(header::CONTENT_RANGE, v);
    }
    resp
}

/// `attachment` with an ASCII-only quoted `filename` fallback and the exact name as RFC 8187
/// `filename*`. Control characters (CR/LF included) and path separators never survive.
fn content_disposition(name: &str) -> HeaderValue {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || c == '/' || c == '\\' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = if clean.is_empty() { "download" } else { &clean };
    let ascii: String = clean
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '%' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::with_capacity(clean.len() * 3);
    for b in clean.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// Status and headers of a file response for `r`, around `body`.
fn file_response(g: &GrantView, r: Resolved, body: Body) -> Response {
    let mut resp = Response::new(body);
    *resp.status_mut() = if r.partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from(r.end - r.start + 1),
    );
    if r.partial
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {}-{}/{}", r.start, r.end, g.size))
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&g.etag) {
        h.insert(header::ETAG, v);
    }
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&g.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        content_disposition(&g.filename),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// Ends the segment if the handler stops (error or client gone) before the pump owns it.
struct SegmentGuard<'a> {
    dd: &'a DdState,
    id: &'a str,
    epoch: u64,
    armed: bool,
}

impl Drop for SegmentGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.dd.grants.end_segment(self.id, self.epoch);
        }
    }
}

/// Open `[r.start, r.end]` upstream, retrying once (after a short pause) when the backend is
/// unavailable. Every unavailable attempt counts as an upstream error.
async fn open_upstream(
    dd: &DdState,
    g: &GrantView,
    r: Resolved,
) -> Result<reqwest::Response, BackendError> {
    let mut retried = false;
    loop {
        match dd.backends.open(g.backend, &g.file, r.start, r.end).await {
            Err(BackendError::Unavailable(e)) => {
                dd.stats.record_upstream_error();
                if retried {
                    return Err(BackendError::Unavailable(e));
                }
                retried = true;
                tokio::time::sleep(OPEN_RETRY_DELAY).await;
            }
            other => return other,
        }
    }
}

/// `GET /api/v1/downloads/{id}/file`: the bytes, with `Range` / `If-Range`.
///
/// `HEAD` answers the same headers without opening a stream (it neither preempts the
/// current stream nor counts against the request rate guard).
pub async fn file(
    State(state): State<AppState>,
    method: Method,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: TokenParam,
) -> Result<Response, DdError> {
    let dd = enabled(&state)?;
    let g = dd
        .grants
        .authorize(&id, download_token(&headers, &query)?, now_ms())?;
    let range = requested_range(&headers, &g.etag);
    let Ok(r) = ranges::resolve(range, g.size, dd.cfg.max_range_bytes) else {
        return Ok(unsatisfiable(g.size));
    };
    if method == Method::HEAD {
        return Ok(file_response(&g, r, Body::empty()));
    }

    let segment = dd.grants.begin_segment(&id, now_ms())?;
    let mut guard = SegmentGuard {
        dd: &dd,
        id: &id,
        epoch: segment.epoch,
        armed: true,
    };
    let first = match open_upstream(&dd, &g, r).await {
        Ok(first) => first,
        Err(e) => {
            drop(guard);
            if matches!(e, BackendError::SourceChanged | BackendError::NotFound) {
                // The grant can never be served again: free its slot now.
                dd.grants.fail_source_changed(&id, now_ms());
            }
            return Err(backend_error(g.backend, e));
        }
    };
    guard.armed = false;
    drop(guard);
    let rx = pump::spawn(PumpParams {
        dd: dd.clone(),
        grant_id: id.clone(),
        kind: g.backend,
        file: g.file.clone(),
        segment,
        start: r.start,
        end: r.end,
        first,
    });
    let body = Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }));
    Ok(file_response(&g, r, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cd(name: &str) -> String {
        content_disposition(name).to_str().unwrap().to_owned()
    }

    #[test]
    fn content_disposition_plain_name() {
        assert_eq!(
            cd("Movie One (2020).mkv"),
            "attachment; filename=\"Movie One (2020).mkv\"; \
             filename*=UTF-8''Movie%20One%20%282020%29.mkv"
        );
    }

    #[test]
    fn content_disposition_quotes_backslashes_and_percent() {
        assert_eq!(
            cd(r#"a"b\c%d.mkv"#),
            "attachment; filename=\"a_b_c_d.mkv\"; filename*=UTF-8''a%22b_c%25d.mkv"
        );
    }

    #[test]
    fn content_disposition_non_ascii_is_encoded_in_utf8() {
        assert_eq!(
            cd("Amélie 日本.mkv"),
            "attachment; filename=\"Am_lie __.mkv\"; \
             filename*=UTF-8''Am%C3%A9lie%20%E6%97%A5%E6%9C%AC.mkv"
        );
    }

    #[test]
    fn content_disposition_control_chars_cannot_inject_headers() {
        let v = cd("x.mkv\r\nSet-Cookie: a=b\0\t");
        assert!(
            !v.contains('\r') && !v.contains('\n') && !v.contains('\0'),
            "{v}"
        );
        assert_eq!(
            v,
            "attachment; filename=\"x.mkv__Set-Cookie: a=b_\"; \
             filename*=UTF-8''x.mkv__Set-Cookie%3A%20a%3Db_"
        );
        assert!(!cd("a/../b.mkv").contains('/'));
    }

    #[test]
    fn content_disposition_empty_name_falls_back() {
        assert_eq!(
            cd(" \r\n "),
            "attachment; filename=\"download\"; filename*=UTF-8''download"
        );
    }

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(clean_title(Some("  a b  ".into())).as_deref(), Some("a b"));
        assert_eq!(clean_title(Some("   ".into())), None);
        assert_eq!(clean_title(None), None);
        let long = "é".repeat(300);
        assert_eq!(clean_title(Some(long)).unwrap().chars().count(), 200);
    }

    #[test]
    fn if_range_must_match_the_strong_etag() {
        let mut h = HeaderMap::new();
        assert_eq!(requested_range(&h, "\"e\""), None);
        h.insert(header::RANGE, HeaderValue::from_static("bytes=1-2"));
        assert_eq!(requested_range(&h, "\"e\""), Some("bytes=1-2"));
        h.insert(header::IF_RANGE, HeaderValue::from_static("\"e\""));
        assert_eq!(requested_range(&h, "\"e\""), Some("bytes=1-2"));
        for stale in ["\"f\"", "W/\"e\"", "Wed, 21 Oct 2015 07:28:00 GMT"] {
            h.insert(header::IF_RANGE, HeaderValue::from_static(stale));
            assert_eq!(requested_range(&h, "\"e\""), None, "{stale}");
        }
    }
}

//! A fake Jellyfin + Plex server on an ephemeral port, serving one item (`movie1`) whose
//! byte `i` is `(i % 251) as u8`, with knobs to simulate replaced files, upstream cuts and
//! slow first bytes.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

pub const ITEM: &str = "movie1";
pub const JF_KEY: &str = "jf-key";
pub const PLEX_TOKEN: &str = "plex-token";
pub const FILE_PATH: &str = "/media/movies/Movie One (2020).mkv";
const BODY_CHUNK: u64 = 16 * 1024;

#[derive(Default)]
pub struct FakeKnobs {
    /// Current size of the file (change it to simulate a replaced file).
    pub size: AtomicU64,
    /// 0 = off; when `n > 0` the next file response body is cut after `n` bytes (then resets).
    pub cut_after: AtomicU64,
    /// Delay before any response headers are sent.
    pub delay_first_byte_ms: AtomicU64,
    /// Number of file (byte) requests served, metadata requests excluded.
    pub requests: AtomicU64,
    /// `Range` header of the last file request.
    pub last_range: Mutex<Option<String>>,
    /// File response bodies currently alive on the server side.
    pub open_bodies: AtomicI64,
}

pub struct FakeMedia {
    pub url: String,
    pub knobs: Arc<FakeKnobs>,
}

/// The bytes `[start, start + len)` of the fake file.
pub fn expected(start: u64, len: u64) -> Vec<u8> {
    (start..start + len).map(|i| (i % 251) as u8).collect()
}

impl FakeMedia {
    pub async fn start(size: u64) -> FakeMedia {
        let knobs = Arc::new(FakeKnobs::default());
        knobs.size.store(size, Ordering::SeqCst);
        let app = Router::new()
            .route("/Items", get(jf_items))
            .route("/Items/{id}/Download", get(jf_download))
            .route("/library/metadata/{id}", get(plex_metadata))
            .route("/library/parts/1/{ts}/file.mkv", get(plex_part))
            .with_state(knobs.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeMedia {
            url: format!("http://{addr}"),
            knobs,
        }
    }

    /// `FLICKDD_*` variables pointing both backends at this server with the right secrets.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![
            ("FLICKDD_ENABLED".into(), "true".into()),
            ("FLICKDD_JELLYFIN_URL".into(), self.url.clone()),
            ("FLICKDD_JELLYFIN_API_KEY".into(), JF_KEY.into()),
            ("FLICKDD_PLEX_URL".into(), self.url.clone()),
            ("FLICKDD_PLEX_TOKEN".into(), PLEX_TOKEN.into()),
        ]
    }

    pub fn size(&self) -> u64 {
        self.knobs.size.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> u64 {
        self.knobs.requests.load(Ordering::SeqCst)
    }

    pub fn open_bodies(&self) -> i64 {
        self.knobs.open_bodies.load(Ordering::SeqCst)
    }

    pub fn last_range(&self) -> Option<String> {
        self.knobs.last_range.lock().unwrap().clone()
    }
}

type Knobs = State<Arc<FakeKnobs>>;

fn authorized(headers: &HeaderMap, name: &str, secret: &str) -> bool {
    headers.get(name).and_then(|v| v.to_str().ok()) == Some(secret)
}

async fn first_byte_delay(k: &FakeKnobs) {
    let ms = k.delay_first_byte_ms.load(Ordering::SeqCst);
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

async fn jf_items(
    State(k): Knobs,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    first_byte_delay(&k).await;
    if !authorized(&headers, "x-emby-token", JF_KEY) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let size = k.size.load(Ordering::SeqCst);
    let items = if q.get("ids").map(String::as_str) == Some(ITEM) {
        vec![json!({
            "Id": ITEM,
            "Name": "Movie One",
            "Path": FILE_PATH,
            "MediaSources": [{
                "Id": "ms1",
                "Path": FILE_PATH,
                "Container": "mkv",
                "Size": size,
                "ETag": format!("etag-{size}"),
            }],
        })]
    } else {
        vec![]
    };
    let n = items.len();
    axum::Json(json!({ "Items": items, "TotalRecordCount": n })).into_response()
}

async fn jf_download(State(k): Knobs, headers: HeaderMap, Path(id): Path<String>) -> Response {
    first_byte_delay(&k).await;
    if !authorized(&headers, "x-emby-token", JF_KEY) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if id != ITEM {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve_file(&k, &headers)
}

async fn plex_metadata(State(k): Knobs, headers: HeaderMap, Path(id): Path<String>) -> Response {
    first_byte_delay(&k).await;
    if !authorized(&headers, "x-plex-token", PLEX_TOKEN) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if id != ITEM {
        return StatusCode::NOT_FOUND.into_response();
    }
    let size = k.size.load(Ordering::SeqCst);
    // The part key embeds an "update timestamp": here the size, which changes with the file.
    axum::Json(json!({
        "MediaContainer": {
            "size": 1,
            "Metadata": [{
                "ratingKey": ITEM,
                "title": "Movie One",
                "Media": [{
                    "container": "mkv",
                    "Part": [{
                        "id": 1,
                        "key": format!("/library/parts/1/{size}/file.mkv"),
                        "file": "/data/movies/Movie One (2020).mkv",
                        "size": size,
                        "container": "mkv",
                    }],
                }],
            }],
        }
    }))
    .into_response()
}

async fn plex_part(State(k): Knobs, headers: HeaderMap, Path(_ts): Path<String>) -> Response {
    first_byte_delay(&k).await;
    if !authorized(&headers, "x-plex-token", PLEX_TOKEN) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    serve_file(&k, &headers)
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` against `size`: inclusive `(start, end)`, or `None`
/// when unsatisfiable.
fn parse_range(v: &str, size: u64) -> Option<(u64, u64)> {
    let spec = v.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (size.saturating_sub(n), size.checked_sub(1)?)
        }
        (a, "") => (a.parse().ok()?, size.checked_sub(1)?),
        (a, b) => (
            a.parse().ok()?,
            b.parse::<u64>().ok()?.min(size.checked_sub(1)?),
        ),
    };
    (start <= end && start < size).then_some((start, end))
}

/// Decrements `open_bodies` when the server drops a response body.
struct BodyGuard(Arc<FakeKnobs>);

impl Drop for BodyGuard {
    fn drop(&mut self) {
        self.0.open_bodies.fetch_sub(1, Ordering::SeqCst);
    }
}

fn serve_file(k: &Arc<FakeKnobs>, headers: &HeaderMap) -> Response {
    k.requests.fetch_add(1, Ordering::SeqCst);
    let size = k.size.load(Ordering::SeqCst);
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    *k.last_range.lock().unwrap() = range.clone();
    let (status, start, end) = match &range {
        None if size == 0 => return StatusCode::OK.into_response(),
        None => (StatusCode::OK, 0, size - 1),
        Some(r) => match parse_range(r, size) {
            Some((s, e)) => (StatusCode::PARTIAL_CONTENT, s, e),
            None => {
                let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
                );
                return resp;
            }
        },
    };
    let len = end - start + 1;
    let cut = k.cut_after.swap(0, Ordering::SeqCst);
    let limit = if cut > 0 { cut.min(len) } else { len };
    k.open_bodies.fetch_add(1, Ordering::SeqCst);
    let guard = BodyGuard(k.clone());
    // State: (next offset, bytes left before the cut, the guard).
    let stream = futures_util::stream::unfold(
        (start, limit, cut > 0, Some(guard)),
        move |(pos, left, cutting, guard)| async move {
            let guard = guard?;
            if left == 0 {
                if cutting {
                    // Let hyper flush the headers and the bytes before the cut (it flushes
                    // when the body is pending), then abort: the client sees a truncated
                    // response rather than no response at all.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let err: io::Result<Bytes> = Err(io::Error::other("cut by the fake server"));
                    return Some((err, (pos, 0, false, None)));
                }
                return None;
            }
            let n = left.min(BODY_CHUNK);
            let bytes = Bytes::from(expected(pos, n));
            Some((Ok(bytes), (pos + n, left - n, cutting, Some(guard))))
        },
    );
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("video/x-matroska"),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).unwrap(),
        );
    }
    resp
}

//! Media backends (Jellyfin, Plex): resolve an item to its original file, open byte ranges.
//!
//! Item ids are validated before they reach a URL; secrets travel in headers only, so
//! error messages (which may carry the URL) never leak them.

use std::time::Duration;

use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, RANGE};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::config::{BackendConfig, DdConfig};
pub use super::types::{BackendKind, ResolvedFile};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("item not found")]
    NotFound,
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    #[error("source file changed")]
    SourceChanged,
    #[error("backend not configured")]
    NotConfigured,
}

/// `^[A-Za-z0-9_-]{1,64}$`: no path or query injection towards the backend.
pub fn valid_item_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub struct Backends {
    client: Client,
    jellyfin: Option<BackendConfig>,
    plex: Option<BackendConfig>,
    upstream_timeout: Duration,
}

// --- Jellyfin / Plex JSON (only the fields we read) ------------------------------------

#[derive(Deserialize)]
struct JfItems {
    #[serde(rename = "Items", default)]
    items: Vec<JfItem>,
}

#[derive(Deserialize)]
struct JfItem {
    #[serde(rename = "MediaSources", default)]
    media_sources: Vec<JfSource>,
}

#[derive(Deserialize)]
struct JfSource {
    #[serde(rename = "Size")]
    size: Option<u64>,
    #[serde(rename = "Container")]
    container: Option<String>,
    #[serde(rename = "Path")]
    path: Option<String>,
    #[serde(rename = "ETag")]
    etag: Option<String>,
}

#[derive(Deserialize)]
struct PlexEnvelope {
    #[serde(rename = "MediaContainer")]
    container: PlexContainer,
}

#[derive(Deserialize)]
struct PlexContainer {
    #[serde(rename = "Metadata", default)]
    metadata: Vec<PlexMetadata>,
}

#[derive(Deserialize)]
struct PlexMetadata {
    #[serde(rename = "Media", default)]
    media: Vec<PlexMedia>,
}

#[derive(Deserialize)]
struct PlexMedia {
    #[serde(rename = "Part", default)]
    parts: Vec<PlexPart>,
}

#[derive(Deserialize)]
struct PlexPart {
    key: Option<String>,
    size: Option<u64>,
    file: Option<String>,
    container: Option<String>,
}

fn unavailable(e: impl std::fmt::Display) -> BackendError {
    BackendError::Unavailable(e.to_string())
}

/// Last segment of a backend file path (`/` or `\` separated), if any.
fn file_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn mime_of(ext: &str) -> Option<&'static str> {
    Some(match ext.trim().to_ascii_lowercase().as_str() {
        "mkv" | "matroska" => "video/x-matroska",
        "mp4" | "m4v" => "video/mp4",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "ts" => "video/mp2t",
        _ => return None,
    })
}

/// Mime type from the container, falling back to the file extension. A container list
/// (ffprobe style, e.g. `mov,mp4,m4a`) is ambiguous, so there the extension wins.
fn mime_for(container: &str, filename: &str) -> String {
    let by_ext = filename.rsplit_once('.').and_then(|(_, ext)| mime_of(ext));
    let by_container = || container.split(',').find_map(mime_of);
    let mime = if container.contains(',') {
        by_ext.or_else(by_container)
    } else {
        by_container().or(by_ext)
    };
    mime.unwrap_or("application/octet-stream").to_owned()
}

fn etag_for(kind: BackendKind, id: &str, size: u64, source: &str) -> String {
    let digest = Sha256::digest(format!("{}|{id}|{size}|{source}", kind.as_str()).as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

/// Common tail of `resolve`: filename/mime fallbacks, empty-file guard, etag.
fn resolved(
    kind: BackendKind,
    id: &str,
    size: Option<u64>,
    container: Option<String>,
    file_path: Option<String>,
    source_tag: String,
    path: String,
) -> Result<ResolvedFile, BackendError> {
    let size = size.ok_or_else(|| unavailable("the backend reports no file size"))?;
    if size == 0 {
        return Err(unavailable("empty file"));
    }
    let container = container.unwrap_or_default();
    let first = container.split(',').next().unwrap_or("").trim();
    let filename = file_path.as_deref().and_then(file_name).unwrap_or_else(|| {
        if first.is_empty() {
            id.to_owned()
        } else {
            format!("{id}.{first}")
        }
    });
    Ok(ResolvedFile {
        size,
        mime: mime_for(&container, &filename),
        filename,
        etag: etag_for(kind, id, size, &source_tag),
        path,
    })
}

/// `Content-Range: bytes a-b/total` or `bytes */total`: `(range, total)`.
fn parse_content_range(v: &str) -> (Option<(u64, u64)>, Option<u64>) {
    let Some(rest) = v.trim().strip_prefix("bytes ") else {
        return (None, None);
    };
    let Some((range, total)) = rest.split_once('/') else {
        return (None, None);
    };
    let range = range
        .split_once('-')
        .and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
    (range, total.trim().parse().ok())
}

fn header_u64(h: &HeaderMap, name: reqwest::header::HeaderName) -> Option<u64> {
    h.get(name)?.to_str().ok()?.trim().parse().ok()
}

impl Backends {
    /// HTTP client without redirects, 5 s connect timeout and no total timeout (downloads
    /// are long); the first byte is bounded by `upstream_timeout_ms` per request.
    pub fn new(cfg: &DdConfig) -> Self {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("failed to build the FlickDD HTTP client");
        Self {
            client,
            jellyfin: cfg.jellyfin.clone(),
            plex: cfg.plex.clone(),
            upstream_timeout: Duration::from_millis(cfg.upstream_timeout_ms),
        }
    }

    pub fn has(&self, kind: BackendKind) -> bool {
        self.config(kind).is_some()
    }

    fn config(&self, kind: BackendKind) -> Option<&BackendConfig> {
        match kind {
            BackendKind::Jellyfin => self.jellyfin.as_ref(),
            BackendKind::Plex => self.plex.as_ref(),
        }
    }

    fn get(&self, kind: BackendKind, cfg: &BackendConfig, path_and_query: &str) -> RequestBuilder {
        let rb = self.client.get(format!("{}{path_and_query}", cfg.url));
        match kind {
            BackendKind::Jellyfin => rb.header("X-Emby-Token", &cfg.secret),
            BackendKind::Plex => rb
                .header("X-Plex-Token", &cfg.secret)
                .header("Accept", "application/json"),
        }
    }

    /// Send with the first-byte timeout (headers received).
    async fn send(&self, rb: RequestBuilder) -> Result<Response, BackendError> {
        match tokio::time::timeout(self.upstream_timeout, rb.send()).await {
            Err(_) => Err(unavailable("timed out waiting for the backend")),
            Ok(Err(e)) => Err(unavailable(e.without_url())),
            Ok(Ok(r)) => Ok(r),
        }
    }

    /// Metadata call: status mapping, then the JSON body (bounded by the same timeout).
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        kind: BackendKind,
        cfg: &BackendConfig,
        path_and_query: &str,
    ) -> Result<T, BackendError> {
        let resp = self.send(self.get(kind, cfg, path_and_query)).await?;
        match resp.status() {
            s if s.is_success() => {}
            StatusCode::NOT_FOUND => return Err(BackendError::NotFound),
            s => return Err(unavailable(format!("{} answered {s}", kind.as_str()))),
        }
        let body = tokio::time::timeout(self.upstream_timeout, resp.bytes())
            .await
            .map_err(|_| unavailable("timed out reading the backend answer"))?
            .map_err(|e| unavailable(e.without_url()))?;
        serde_json::from_slice(&body)
            .map_err(|e| unavailable(format!("unexpected {} answer: {e}", kind.as_str())))
    }

    pub async fn resolve(
        &self,
        kind: BackendKind,
        item_id: &str,
    ) -> Result<ResolvedFile, BackendError> {
        let cfg = self.config(kind).ok_or(BackendError::NotConfigured)?;
        if !valid_item_id(item_id) {
            return Err(BackendError::NotFound);
        }
        match kind {
            BackendKind::Jellyfin => {
                let items: JfItems = self
                    .get_json(
                        kind,
                        cfg,
                        &format!("/Items?ids={item_id}&Fields=MediaSources,Path"),
                    )
                    .await?;
                let src = items
                    .items
                    .into_iter()
                    .next()
                    .and_then(|i| i.media_sources.into_iter().next())
                    .ok_or(BackendError::NotFound)?;
                let tag = src
                    .etag
                    .clone()
                    .or_else(|| src.path.clone())
                    .unwrap_or_default();
                resolved(
                    kind,
                    item_id,
                    src.size,
                    src.container,
                    src.path,
                    tag,
                    format!("/Items/{item_id}/Download"),
                )
            }
            BackendKind::Plex => {
                let env: PlexEnvelope = self
                    .get_json(kind, cfg, &format!("/library/metadata/{item_id}"))
                    .await?;
                let part = env
                    .container
                    .metadata
                    .into_iter()
                    .next()
                    .and_then(|m| m.media.into_iter().next())
                    .and_then(|m| m.parts.into_iter().next())
                    .ok_or(BackendError::NotFound)?;
                let key = part
                    .key
                    .filter(|k| k.starts_with('/'))
                    .ok_or_else(|| unavailable("plex part has no usable key"))?;
                resolved(
                    kind,
                    item_id,
                    part.size,
                    part.container,
                    part.file,
                    key.clone(),
                    key,
                )
            }
        }
    }

    /// Ranged request of `[start, end]` (inclusive). Verifies status 206/200 and that the
    /// declared total/length matches `file.size`.
    pub async fn open(
        &self,
        kind: BackendKind,
        file: &ResolvedFile,
        start: u64,
        end: u64,
    ) -> Result<Response, BackendError> {
        let cfg = self.config(kind).ok_or(BackendError::NotConfigured)?;
        if start > end || end >= file.size {
            return Err(unavailable("range outside the file"));
        }
        let rb = self
            .get(kind, cfg, &file.path)
            .header(RANGE, format!("bytes={start}-{end}"));
        let resp = self.send(rb).await?;
        let status = resp.status();
        let headers = resp.headers();
        let (range, total) = headers
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .map(parse_content_range)
            .unwrap_or((None, None));
        if total.is_some_and(|t| t != file.size) {
            return Err(BackendError::SourceChanged);
        }
        let len = end - start + 1;
        match status {
            StatusCode::PARTIAL_CONTENT => {
                if range.is_some_and(|r| r != (start, end)) {
                    return Err(unavailable("the backend answered another range"));
                }
            }
            StatusCode::OK if start == 0 && len == file.size => {}
            StatusCode::NOT_FOUND => return Err(BackendError::NotFound),
            s => return Err(unavailable(format!("{} answered {s}", kind.as_str()))),
        }
        if header_u64(headers, CONTENT_LENGTH).is_some_and(|l| l != len) {
            return Err(BackendError::SourceChanged);
        }
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_and_mime_types() {
        assert_eq!(file_name("/a/b/Movie.mkv").as_deref(), Some("Movie.mkv"));
        assert_eq!(
            file_name("D:\\Films\\X (1999).mp4").as_deref(),
            Some("X (1999).mp4")
        );
        assert_eq!(file_name("/a/b/"), None);
        assert_eq!(mime_for("matroska", "x"), "video/x-matroska");
        assert_eq!(mime_for("mov,mp4,m4a", "x"), "video/quicktime");
        assert_eq!(mime_for("mov,mp4,m4a", "x.mp4"), "video/mp4");
        assert_eq!(mime_for("mkv", "x.avi"), "video/x-matroska");
        assert_eq!(mime_for("m4v", "x"), "video/mp4");
        assert_eq!(mime_for("avi", "x"), "video/x-msvideo");
        assert_eq!(mime_for("webm", "x"), "video/webm");
        assert_eq!(mime_for("ts", "x"), "video/mp2t");
        assert_eq!(mime_for("", "film.MKV"), "video/x-matroska");
        assert_eq!(mime_for("flv", "film.flv"), "application/octet-stream");
    }

    #[test]
    fn resolved_falls_back_to_id_dot_container_and_rejects_empty_files() {
        let f = resolved(
            BackendKind::Jellyfin,
            "abc",
            Some(5),
            Some("mkv,webm".into()),
            None,
            String::new(),
            "/p".into(),
        )
        .unwrap();
        assert_eq!(
            (f.filename.as_str(), f.mime.as_str()),
            ("abc.mkv", "video/x-matroska")
        );
        assert!(matches!(
            resolved(
                BackendKind::Plex,
                "a",
                Some(0),
                None,
                None,
                String::new(),
                "/p".into()
            ),
            Err(BackendError::Unavailable(_))
        ));
        assert!(matches!(
            resolved(
                BackendKind::Plex,
                "a",
                None,
                None,
                None,
                String::new(),
                "/p".into()
            ),
            Err(BackendError::Unavailable(_))
        ));
    }

    #[test]
    fn etag_is_quoted_32_hex_and_depends_on_every_input() {
        let e = etag_for(BackendKind::Plex, "a", 1, "s");
        assert_eq!(e.len(), 34);
        assert!(e[1..33].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(e, etag_for(BackendKind::Jellyfin, "a", 1, "s"));
        assert_ne!(e, etag_for(BackendKind::Plex, "b", 1, "s"));
        assert_ne!(e, etag_for(BackendKind::Plex, "a", 2, "s"));
        assert_ne!(e, etag_for(BackendKind::Plex, "a", 1, "t"));
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(
            parse_content_range("bytes 0-9/100"),
            (Some((0, 9)), Some(100))
        );
        assert_eq!(parse_content_range("bytes */100"), (None, Some(100)));
        assert_eq!(parse_content_range("bytes 0-9/*"), (Some((0, 9)), None));
        assert_eq!(parse_content_range("junk"), (None, None));
    }

    #[test]
    fn hostile_plex_keys_cannot_change_the_request_host() {
        // The key is appended to the configured base URL (never parsed as a reference), so
        // `//evil.com`, `/\evil.com` and `/@evil.com` stay paths on the backend's authority.
        let cfg = BackendConfig {
            url: "http://plex.local:32400".into(),
            secret: "t".into(),
        };
        let b = Backends::new(&DdConfig::from_lookup(&|_| None).unwrap());
        for key in [
            "//evil.com/x",
            r"/\evil.com",
            "/@evil.com",
            "/@evil.com:1/x",
        ] {
            let req = b.get(BackendKind::Plex, &cfg, key).build().unwrap();
            let url = req.url();
            assert_eq!(url.host_str(), Some("plex.local"), "{key}");
            assert_eq!(url.port(), Some(32400), "{key}");
            assert_eq!(url.username(), "", "{key}");
        }
    }
}

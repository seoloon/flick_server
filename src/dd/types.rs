//! Types shared by the grant registry, the analytics and the media backends.

use serde::Serialize;

/// Media server a file is downloaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Jellyfin,
    Plex,
}

impl BackendKind {
    pub const ALL: [BackendKind; 2] = [BackendKind::Jellyfin, BackendKind::Plex];

    pub fn as_str(&self) -> &'static str {
        match self {
            BackendKind::Jellyfin => "jellyfin",
            BackendKind::Plex => "plex",
        }
    }
}

/// A media file resolved on a backend, frozen for the lifetime of a grant.
#[derive(Debug, Clone)]
pub struct ResolvedFile {
    pub size: u64,
    pub filename: String,
    pub mime: String,
    pub etag: String,
    /// Backend-relative path of the raw file (e.g. `/Items/{id}/Download`).
    pub path: String,
}

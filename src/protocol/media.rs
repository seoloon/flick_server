//! Media reference.
//!
//! FlickSync never resolves, fetches or proxies media. A `MediaRef` is an
//! opaque identity that each Flick client resolves through its *own*
//! Jellyfin/Plex connection. Because of that, every field is validated against
//! a strict identifier charset: no URLs, no paths, nothing that could be
//! turned into an SSRF or traversal primitive by a careless consumer.

use serde::{Deserialize, Serialize};

use crate::chat::sanitize_text;
use crate::errors::{Error, ErrorCode, Result};

const MAX_ID_LEN: usize = 128;
const MAX_TITLE_CHARS: usize = 200;
const MAX_DURATION_SECS: f64 = 7.0 * 24.0 * 3600.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Jellyfin,
    Plex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Movie,
    Episode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub provider: Provider,
    /// Identifier of the Jellyfin/Plex server (not the Flick server).
    pub server_id: String,
    pub media_id: String,
    pub media_type: MediaType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    /// Display-only hint; never used for resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional, lets the server clamp the canonical position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidMedia, msg)
}

impl MediaRef {
    /// Validate and normalise (sanitises the title).
    pub fn validate(mut self) -> Result<Self> {
        for (name, v) in [
            ("server_id", Some(&self.server_id)),
            ("media_id", Some(&self.media_id)),
        ] {
            if !v.is_some_and(|s| valid_id(s)) {
                return Err(bad(format!(
                    "{name} must be 1-{MAX_ID_LEN} chars of [A-Za-z0-9._:-]"
                )));
            }
        }
        for (name, v) in [
            ("season_id", &self.season_id),
            ("episode_id", &self.episode_id),
        ] {
            if v.as_deref().is_some_and(|s| !valid_id(s)) {
                return Err(bad(format!(
                    "{name} must be 1-{MAX_ID_LEN} chars of [A-Za-z0-9._:-]"
                )));
            }
        }
        if self.media_type == MediaType::Movie
            && (self.season_id.is_some() || self.episode_id.is_some())
        {
            return Err(bad("season_id/episode_id are only valid for episodes"));
        }
        if let Some(d) = self.duration_secs
            && (!d.is_finite() || d <= 0.0 || d > MAX_DURATION_SECS)
        {
            return Err(bad("duration_secs out of range"));
        }
        self.title = self
            .title
            .map(|t| sanitize_text(&t, MAX_TITLE_CHARS))
            .filter(|t| !t.is_empty());
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie() -> MediaRef {
        MediaRef {
            provider: Provider::Jellyfin,
            server_id: "abc123".into(),
            media_id: "7f3e9a1c2b".into(),
            media_type: MediaType::Movie,
            season_id: None,
            episode_id: None,
            title: Some("  Heat\u{0007} ".into()),
            duration_secs: Some(10200.0),
        }
    }

    #[test]
    fn valid_movie_is_accepted_and_title_sanitised() {
        let m = movie().validate().unwrap();
        assert_eq!(m.title.as_deref(), Some("Heat"));
    }

    #[test]
    fn urls_and_paths_are_rejected() {
        for bad_id in [
            "http://evil/x",
            "../../etc/passwd",
            "a/b",
            "a b",
            "",
            "a\\b",
            "id?x=1",
        ] {
            let mut m = movie();
            m.media_id = bad_id.into();
            assert_eq!(
                m.validate().unwrap_err().code,
                ErrorCode::InvalidMedia,
                "{bad_id}"
            );
        }
        let mut m = movie();
        m.server_id = "x".repeat(129);
        assert!(m.validate().is_err());
    }

    #[test]
    fn season_episode_rules() {
        let mut m = movie();
        m.season_id = Some("s1".into());
        assert!(m.validate().is_err(), "movies have no season");
        let mut e = movie();
        e.media_type = MediaType::Episode;
        e.season_id = Some("s1".into());
        e.episode_id = Some("e1".into());
        assert!(e.validate().is_ok());
    }

    #[test]
    fn bad_duration_rejected() {
        for d in [0.0, -5.0, 1e12] {
            let mut m = movie();
            m.duration_secs = Some(d);
            assert!(m.validate().is_err());
        }
    }

    #[test]
    fn plex_roundtrip() {
        let json = r#"{"provider":"plex","server_id":"m1","media_id":"4521","media_type":"episode","season_id":"s2","episode_id":"4521"}"#;
        let m: MediaRef = serde_json::from_str(json).unwrap();
        assert_eq!(m.provider, Provider::Plex);
        assert!(m.validate().is_ok());
    }

    #[test]
    fn unknown_provider_fails_to_parse() {
        let json = r#"{"provider":"emby","server_id":"m1","media_id":"1","media_type":"movie"}"#;
        assert!(serde_json::from_str::<MediaRef>(json).is_err());
    }
}

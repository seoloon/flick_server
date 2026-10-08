//! The server's latest log lines, kept in memory for the panel's Logs page (`GET /admin/v1/logs`).
//!
//! A `tracing` layer (`LogCapture`) placed after the reloadable level filter records every event
//! the filter lets through into a bounded ring (`LogBuffer`). Lines are lost on restart. Secret
//! values are redacted on the way in and every entry is size-capped, so the buffer never holds
//! more than the console shows and never grows past its capacity.

use std::borrow::Cow;

/// Entries kept when `FLICKSYNC_LOG_BUFFER` is unset.
pub const DEFAULT_CAPACITY: usize = 2000;
/// Largest `FLICKSYNC_LOG_BUFFER` accepted.
pub const MAX_CAPACITY: usize = 10_000;
/// `message` is cut beyond this many bytes (then `…`).
pub const MAX_MESSAGE_BYTES: usize = 2048;
/// `fields` is cut beyond this many bytes (then `…`).
pub const MAX_FIELDS_BYTES: usize = 1024;
/// Entries per answer when the client gives no `limit`.
pub const DEFAULT_LIMIT: usize = 500;
/// Most entries per answer.
pub const MAX_LIMIT: usize = 1000;
/// What replaces a secret value: the marker `Config`'s `Debug` already uses.
pub const REDACTED: &str = "<redacted>";

/// Severity, least severe first, so `>=` reads "at least as severe as".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// `error`, `warn`, `info`, `debug` or `trace`; case and surrounding spaces ignored.
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            "trace" => Some(Level::Trace),
            _ => None,
        }
    }

    pub fn from_tracing(level: &tracing::Level) -> Level {
        if *level == tracing::Level::ERROR {
            Level::Error
        } else if *level == tracing::Level::WARN {
            Level::Warn
        } else if *level == tracing::Level::INFO {
            Level::Info
        } else if *level == tracing::Level::DEBUG {
            Level::Debug
        } else {
            Level::Trace
        }
    }
}

/// True for a field or parameter name whose value is never recorded.
pub fn is_secret_name(name: &str) -> bool {
    const PARTS: [&str; 9] = [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "authorization",
        "cookie",
        "credential",
    ];
    let n = name.to_ascii_lowercase();
    PARTS.iter().any(|p| n.contains(p))
        || n == "key"
        || n.ends_with("_key")
        || n.ends_with("-key")
        || n.ends_with(".key")
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Where a secret value starting at byte `start` ends: a quoted value at its closing quote
/// (included), otherwise at the first separator.
fn value_end(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    match rest.chars().next() {
        Some(q @ ('"' | '\'')) => rest[1..].find(q).map_or(text.len(), |p| start + 1 + p + 1),
        _ => rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ',' | ';' | ')' | ']' | '}')
            })
            .map_or(text.len(), |p| start + p),
    }
}

/// `text` with the value after `Bearer ` and the value of every `name=value` whose name is a
/// secret name replaced by [`REDACTED`]. Borrowed when nothing changes.
pub fn scrub(text: &str) -> Cow<'_, str> {
    const BEARER: &str = "bearer ";
    let has_bearer = text
        .as_bytes()
        .windows(BEARER.len())
        .any(|w| w.eq_ignore_ascii_case(BEARER.as_bytes()));
    if !has_bearer && !text.contains('=') {
        return Cow::Borrowed(text);
    }
    // ASCII lowercasing keeps every byte offset, so indices found in `lower` are valid in `text`.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    while from < text.len() {
        let eq = lower[from..].find('=').map(|p| from + p);
        let bearer = lower[from..].find(BEARER).map(|p| from + p);
        let value_start = match (eq, bearer) {
            (None, None) => break,
            (Some(e), Some(b)) if b < e => b + BEARER.len(),
            (None, Some(b)) => b + BEARER.len(),
            (Some(e), _) => {
                let name_start = text[..e]
                    .char_indices()
                    .rev()
                    .find(|&(_, c)| !is_name_char(c))
                    .map_or(0, |(i, c)| i + c.len_utf8());
                if name_start == e || !is_secret_name(&text[name_start..e]) {
                    from = e + 1;
                    continue;
                }
                e + 1
            }
        };
        if text[value_start..].starts_with(REDACTED) {
            from = value_start + REDACTED.len();
            continue;
        }
        let end = value_end(text, value_start);
        if end > value_start {
            out.push_str(&text[copied..value_start]);
            out.push_str(REDACTED);
            copied = end;
        }
        from = end.max(value_start);
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// `s` cut to at most `max` bytes on a character boundary, with `…` appended when cut.
fn truncated(s: Cow<'_, str>, max: usize) -> String {
    if s.len() <= max {
        return s.into_owned();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(cut + '…'.len_utf8());
    out.push_str(&s[..cut]);
    out.push('…');
    out
}

/// What the buffer stores for a message or a fields text: scrubbed first, then capped.
pub fn clean(text: &str, max: usize) -> String {
    truncated(scrub(text), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_order_by_severity() {
        assert_eq!(Level::parse(" WARN "), Some(Level::Warn));
        for name in ["error", "warn", "info", "debug", "trace"] {
            assert!(Level::parse(name).is_some(), "{name}");
        }
        assert_eq!(Level::parse("loud"), None);
        assert_eq!(Level::parse(""), None);
        assert!(Level::Error > Level::Warn);
        assert!(Level::Warn > Level::Info);
        assert!(Level::Info > Level::Debug);
        assert!(Level::Debug > Level::Trace);
        assert_eq!(Level::from_tracing(&tracing::Level::WARN), Level::Warn);
        assert_eq!(Level::from_tracing(&tracing::Level::TRACE), Level::Trace);
        assert_eq!(serde_json::to_string(&Level::Error).unwrap(), "\"error\"");
    }

    #[test]
    fn secret_names_are_recognised() {
        for n in [
            "token",
            "access_token",
            "X-Plex-Token",
            "api_key",
            "apiKey",
            "password",
            "PANEL_PASSWORD",
            "client_secret",
            "Authorization",
            "cookie",
            "key",
            "signing_key",
            "jwt.key",
            "credentials",
        ] {
            assert!(is_secret_name(n), "{n}");
        }
        for n in [
            "room_id",
            "participant_id",
            "auth_keys",
            "kid",
            "key_count",
            "monkey",
            "error",
            "streams",
            "backend",
        ] {
            assert!(!is_secret_name(n), "{n}");
        }
    }

    #[test]
    fn bearer_values_and_secret_parameters_are_scrubbed() {
        assert_eq!(
            scrub("GET /api/v1/rooms/R/ws?access_token=eyJ.abc.def&x=1 done"),
            "GET /api/v1/rooms/R/ws?access_token=<redacted>&x=1 done"
        );
        assert_eq!(
            scrub("authorization: Bearer abc.DEF-123, next"),
            "authorization: Bearer <redacted>, next"
        );
        assert_eq!(scrub("BEARER xyz"), "BEARER <redacted>");
        assert_eq!(
            scrub("error=Some(\"X-Plex-Token=tok\") api_key=k1 room_id=R1"),
            "error=Some(\"X-Plex-Token=<redacted>\") api_key=<redacted> room_id=R1"
        );
        assert_eq!(
            scrub("password=\"hunter 2\" next"),
            "password=<redacted> next"
        );
        assert_eq!(
            scrub("password=<redacted> user=\"alice\""),
            "password=<redacted> user=\"alice\""
        );
        assert_eq!(scrub("token= trailing token="), "token= trailing token=");
        assert_eq!(scrub("é token=sécret é"), "é token=<redacted> é");
        assert!(matches!(scrub("room created"), Cow::Borrowed(_)));
        assert!(matches!(scrub("room_id=R1 streams=3"), Cow::Borrowed(_)));
    }

    #[test]
    fn long_texts_are_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_MESSAGE_BYTES); // 2 bytes each
        let cut = clean(&long, MAX_MESSAGE_BYTES);
        assert!(cut.ends_with('…'));
        assert!(cut.len() <= MAX_MESSAGE_BYTES + '…'.len_utf8());
        assert_eq!(clean("short", MAX_MESSAGE_BYTES), "short");
        // Byte 4 falls inside the second "é" (bytes 3 and 4): the cut moves back to 3.
        let odd = format!("a{}", "é".repeat(10));
        assert_eq!(clean(&odd, 4), "aé…");
        // Scrubbing happens before the cut, so a secret cannot survive half-cut.
        assert_eq!(clean("token=abcdefgh", 9), "token=<re…");
    }
}

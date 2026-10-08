//! The server's latest log lines, kept in memory for the panel's Logs page (`GET /admin/v1/logs`).
//!
//! A `tracing` layer (`LogCapture`) placed after the reloadable level filter records every event
//! the filter lets through into a bounded ring (`LogBuffer`). Lines are lost on restart. Secret
//! values are redacted on the way in and every entry is size-capped, so the buffer never holds
//! more than the console shows and never grows past its capacity.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Characters that end an unquoted secret value.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ',' | ';' | ')' | ']' | '}')
}

/// Where a secret value starting at byte `start` ends. A quoted value ends after its closing
/// quote: `"..."` (a quote preceded by a backslash does not close it), `'...'`, or the escaped
/// form `\"..."` that `{:?}` produces inside another quoted string. Otherwise it ends at the
/// first separator. An unterminated quote runs to the end of the text.
fn value_end(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    if let Some(inner) = rest.strip_prefix("\\\"") {
        return inner.find("\\\"").map_or(text.len(), |p| start + 2 + p + 2);
    }
    match rest.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let bytes = rest.as_bytes();
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == q as u8 {
                    return start + i + 1;
                } else {
                    i += 1;
                }
            }
            text.len()
        }
        _ => rest.find(is_separator).map_or(text.len(), |p| start + p),
    }
}

const BEARER: &str = "bearer";

/// If `lower` has `bearer` plus whitespace at byte `pos`, where the token after it starts.
fn bearer_value_start(lower: &str, pos: usize) -> Option<usize> {
    let bytes = lower.as_bytes();
    if !lower[pos..].starts_with(BEARER) {
        return None;
    }
    let mut at = pos + BEARER.len();
    if !bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    Some(at)
}

/// Position of the first `Bearer <token>` at or after `from`, with where its token starts.
fn find_bearer(lower: &str, from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while let Some(p) = lower[at..].find(BEARER) {
        let pos = at + p;
        if let Some(value) = bearer_value_start(lower, pos) {
            return Some((pos, value));
        }
        at = pos + BEARER.len();
    }
    None
}

/// `text` with the token after `Bearer` and the value of every `name=value` whose name is a
/// secret name (spaces around `=` allowed) replaced by [`REDACTED`]. A value that is itself
/// `Bearer <token>` is redacted through the token. Borrowed when nothing changes. Linear in
/// the length of `text`.
pub fn scrub(text: &str) -> Cow<'_, str> {
    let has_bearer = text
        .as_bytes()
        .windows(BEARER.len())
        .any(|w| w.eq_ignore_ascii_case(BEARER.as_bytes()));
    if !has_bearer && !text.contains('=') {
        return Cow::Borrowed(text);
    }
    // ASCII lowercasing keeps every byte offset, so indices found in `lower` are valid in `text`.
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut from = 0;
    // Next `=` and next `Bearer`, found once and looked up again only when `from` passes them.
    let mut next_eq: Option<Option<usize>> = None;
    let mut next_bearer: Option<Option<(usize, usize)>> = None;
    while from < text.len() {
        let eq = match next_eq {
            Some(None) => None,
            Some(Some(e)) if e >= from => Some(e),
            _ => {
                let e = lower[from..].find('=').map(|p| from + p);
                next_eq = Some(e);
                e
            }
        };
        let bearer = match next_bearer {
            Some(None) => None,
            Some(Some((p, v))) if p >= from => Some((p, v)),
            _ => {
                let b = find_bearer(&lower, from);
                next_bearer = Some(b);
                b
            }
        };
        let value_start = match (eq, bearer) {
            (None, None) => break,
            (Some(e), Some((p, v))) if p < e => v,
            (None, Some((_, v))) => v,
            (Some(e), _) => {
                let mut name_end = e;
                while name_end > 0 && matches!(bytes[name_end - 1], b' ' | b'\t') {
                    name_end -= 1;
                }
                let name_start = text[..name_end]
                    .char_indices()
                    .rev()
                    .find(|&(_, c)| !is_name_char(c))
                    .map_or(0, |(i, c)| i + c.len_utf8());
                if name_start == name_end || !is_secret_name(&text[name_start..name_end]) {
                    from = e + 1;
                    continue;
                }
                let mut v = e + 1;
                while matches!(bytes.get(v), Some(b' ' | b'\t')) {
                    v += 1;
                }
                bearer_value_start(&lower, v).unwrap_or(v)
            }
        };
        let rest = &text[value_start..];
        if let Some(after) = rest.strip_prefix(REDACTED)
            && after.chars().next().is_none_or(is_separator)
        {
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

/// `s` cut to at most `max` bytes on a character boundary, with `…` appended when cut
/// (or when `was_cut`: the caller already dropped a tail).
fn truncated(s: Cow<'_, str>, max: usize, was_cut: bool) -> String {
    if s.len() <= max {
        let mut out = s.into_owned();
        if was_cut {
            out.push('…');
        }
        return out;
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

/// What the buffer stores for a message or a fields text: scrubbed first, then capped. Only the
/// head of a huge text is scrubbed (the rest is dropped anyway), so the cost is bounded; a
/// secret straddling that head's end is redacted to its end.
pub fn clean(text: &str, max: usize) -> String {
    let limit = max.saturating_mul(4).max(256);
    if text.len() <= limit {
        return truncated(scrub(text), max, false);
    }
    let mut cut = limit;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    truncated(scrub(&text[..cut]), max, true)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One recorded event.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LogEntry {
    /// From 1, +1 per entry, never reused within a run.
    pub seq: u64,
    /// When it was recorded, in ms since the epoch.
    pub ts: u64,
    pub level: Level,
    /// Module path of the code that logged it.
    pub target: String,
    pub message: String,
    /// The event's other fields, `name=value` separated by spaces; "" when none.
    pub fields: String,
}

struct Ring {
    /// Contiguous seqs, oldest first.
    entries: VecDeque<Arc<LogEntry>>,
    next_seq: u64,
}

/// The newest `capacity` log entries of this run.
///
/// Writers (every log event) and readers (the admin route) share one `Mutex` held only to assign
/// a seq and move `Arc`s: building the entry, scrubbing and capping happen before, freeing an
/// evicted entry and serialising happen after.
pub struct LogBuffer {
    ring: Mutex<Ring>,
    capacity: usize,
    boot: u64,
}

impl LogBuffer {
    /// `capacity` above [`MAX_CAPACITY`] is lowered to it; `0` keeps nothing.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.min(MAX_CAPACITY);
        Self {
            ring: Mutex::new(Ring {
                entries: VecDeque::with_capacity(capacity),
                next_seq: 1,
            }),
            capacity,
            boot: now_ms(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Start of this run (ms since the epoch): a change tells a client that seqs started again.
    pub fn boot(&self) -> u64 {
        self.boot
    }

    /// Record one event, secrets scrubbed and texts capped. Returns its seq (0 when capacity is 0).
    pub fn push(&self, level: Level, target: &str, message: &str, fields: &str) -> u64 {
        if self.capacity == 0 {
            return 0;
        }
        let mut entry = Arc::new(LogEntry {
            seq: 0,
            ts: now_ms(),
            level,
            target: target.to_owned(),
            message: clean(message, MAX_MESSAGE_BYTES),
            fields: clean(fields, MAX_FIELDS_BYTES),
        });
        let (seq, evicted) = {
            let mut ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
            let seq = ring.next_seq;
            ring.next_seq += 1;
            Arc::get_mut(&mut entry)
                .expect("the entry is not shared yet")
                .seq = seq;
            let evicted = if ring.entries.len() >= self.capacity {
                ring.entries.pop_front()
            } else {
                None
            };
            ring.entries.push_back(entry);
            (seq, evicted)
        };
        drop(evicted); // freed outside the lock
        seq
    }

    /// The entries after `q.after` that are at least `q.min_level`, oldest first, at most `q.limit`.
    pub fn query(&self, q: &LogQuery) -> LogPage {
        let ring = self.ring.lock().unwrap_or_else(PoisonError::into_inner);
        let first = ring.entries.front().map_or(ring.next_seq, |e| e.seq);
        let dropped = first.saturating_sub(q.after.saturating_add(1));
        // Seqs in the ring are contiguous: the first one after the cursor is found by arithmetic.
        let start = usize::try_from(q.after.saturating_add(1).saturating_sub(first))
            .unwrap_or(usize::MAX)
            .min(ring.entries.len());
        let limit = q.limit.max(1);
        let mut entries = Vec::new();
        let mut next = q.after;
        let mut more = false;
        for e in ring.entries.range(start..) {
            if entries.len() >= limit {
                more = true;
                break;
            }
            next = e.seq;
            if e.level >= q.min_level {
                entries.push(Arc::clone(e));
            }
        }
        LogPage {
            entries,
            next,
            more,
            dropped,
        }
    }
}

/// `GET /admin/v1/logs` parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogQuery {
    /// Return entries with a larger seq.
    pub after: u64,
    /// Least severe level returned.
    pub min_level: Level,
    /// Most entries returned, 1 to [`MAX_LIMIT`].
    pub limit: usize,
}

impl Default for LogQuery {
    fn default() -> Self {
        Self {
            after: 0,
            min_level: Level::Trace,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl LogQuery {
    /// Read `after`, `level` and `limit` from the query string. The error is the message of
    /// `400 INVALID_QUERY`: it names the parameter, the value received and what is expected.
    pub fn from_pairs(pairs: &[(String, String)]) -> Result<Self, String> {
        let mut q = LogQuery::default();
        let mut seen: Vec<&str> = Vec::new();
        for (name, value) in pairs {
            let name = name.as_str();
            if !matches!(name, "after" | "level" | "limit") {
                return Err(format!(
                    "Unknown query parameter '{}'. Valid parameters: after, level, limit.",
                    crate::settings::shorten(name)
                ));
            }
            if seen.contains(&name) {
                return Err(format!(
                    "The query parameter '{name}' is given twice. Give each parameter at most once."
                ));
            }
            seen.push(name);
            let shown = crate::settings::shorten(value);
            match name {
                "after" => {
                    q.after = Some(value.as_str())
                        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| {
                        format!(
                            "after must be a whole number: the seq of the last line you have, or 0 for the oldest line kept; got '{shown}'."
                        )
                    })?;
                }
                "level" => {
                    q.min_level = Level::parse(value).ok_or_else(|| {
                        format!(
                            "level must be one of error, warn, info, debug, trace (the least severe level to return); got '{shown}'."
                        )
                    })?;
                }
                _ => {
                    q.limit = value
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .filter(|n| (1..=MAX_LIMIT).contains(n))
                        .ok_or_else(|| {
                            format!(
                                "limit must be a whole number from 1 to {MAX_LIMIT}; got '{shown}'."
                            )
                        })?;
                }
            }
        }
        Ok(q)
    }
}

/// One answer of the route.
#[derive(Debug)]
pub struct LogPage {
    pub entries: Vec<Arc<LogEntry>>,
    /// The seq of the last entry scanned (filtered-out ones included), or `after` when none was.
    pub next: u64,
    /// The scan stopped at `limit` before the newest entry: ask again with `after = next`.
    pub more: bool,
    /// Entries after the cursor already evicted, whatever their level.
    pub dropped: u64,
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
        assert_eq!(scrub("token= trailing token="), "token= <redacted> token=");
        assert_eq!(scrub("token=&x=1"), "token=&x=1");
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

    #[test]
    fn bearer_values_and_debug_escaped_quotes_do_not_leak() {
        assert_eq!(
            scrub("authorization=Bearer eyJsecret.abc next"),
            "authorization=Bearer <redacted> next"
        );
        assert_eq!(
            scrub("access_token=Bearer x&y=1"),
            "access_token=Bearer <redacted>&y=1"
        );
        assert_eq!(scrub("Bearer\tabc  d"), "Bearer\t<redacted>  d");
        assert_eq!(scrub("Bearer   abc"), "Bearer   <redacted>");
        assert_eq!(scrub("token = x y"), "token = <redacted> y");
        assert_eq!(scrub("token=<redacted>SECRET"), "token=<redacted>");
        let nested = format!("error={:?}", "call failed: token=\"s3cr3t value\" ok");
        let out = scrub(&nested);
        assert!(!out.contains("s3cr3t") && !out.contains("value"), "{out}");
        assert!(out.contains("ok"), "{out}");
        let quoted = format!("password={:?} next", "ab\"cd ef");
        assert_eq!(scrub(&quoted), "password=<redacted> next");
    }

    #[test]
    fn scrubbing_is_linear_on_adversarial_input() {
        let started = std::time::Instant::now();
        let eqs = "a=".repeat(512 * 1024);
        assert!(matches!(scrub(&eqs), Cow::Borrowed(_)));
        let bearers = "a=bearer ".repeat(100_000);
        let _ = scrub(&bearers);
        let nested = "token=\"x ".repeat(100_000);
        let _ = scrub(&nested);
        assert!(clean(&eqs, MAX_MESSAGE_BYTES).len() <= MAX_MESSAGE_BYTES + 3);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[test]
    fn a_cut_head_still_ends_with_an_ellipsis() {
        let huge = format!("{} token={}", "x".repeat(300), "y".repeat(50_000));
        let cut = clean(&huge, 100);
        assert!(cut.ends_with('…') && cut.len() <= 100 + 3, "{cut}");
        assert!(!cut.contains('y'));
    }

    #[test]
    fn limit_zero_still_makes_progress() {
        let b = LogBuffer::new(3);
        b.push(Level::Info, "t", "m", "");
        b.push(Level::Info, "t", "n", "");
        let page = b.query(&LogQuery {
            limit: 0,
            ..LogQuery::default()
        });
        assert_eq!((seqs(&page), page.next, page.more), (vec![1], 1, true));
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn seqs(page: &LogPage) -> Vec<u64> {
        page.entries.iter().map(|e| e.seq).collect()
    }

    #[test]
    fn entries_get_increasing_seqs_and_come_back_after_the_cursor() {
        let b = LogBuffer::new(10);
        assert_eq!(b.push(Level::Info, "t", "one", ""), 1);
        assert_eq!(b.push(Level::Warn, "t", "two", "room_id=R1"), 2);
        assert_eq!(b.push(Level::Error, "t", "three", ""), 3);
        let all = b.query(&LogQuery::default());
        assert_eq!(seqs(&all), [1, 2, 3]);
        assert_eq!((all.next, all.more, all.dropped), (3, false, 0));
        assert_eq!(all.entries[1].message, "two");
        assert_eq!(all.entries[1].fields, "room_id=R1");
        assert_eq!(all.entries[1].level, Level::Warn);
        assert_eq!(all.entries[1].target, "t");
        assert!(all.entries[0].ts > 0);
        let after = b.query(&LogQuery {
            after: 2,
            ..LogQuery::default()
        });
        assert_eq!(seqs(&after), [3]);
        let none = b.query(&LogQuery {
            after: 3,
            ..LogQuery::default()
        });
        assert!(none.entries.is_empty());
        assert_eq!((none.next, none.more), (3, false));
    }

    #[test]
    fn the_level_filter_moves_the_cursor_past_what_it_skips() {
        let b = LogBuffer::new(10);
        b.push(Level::Debug, "t", "d1", "");
        b.push(Level::Warn, "t", "w2", "");
        b.push(Level::Info, "t", "i3", "");
        b.push(Level::Debug, "t", "d4", "");
        let page = b.query(&LogQuery {
            min_level: Level::Info,
            ..LogQuery::default()
        });
        assert_eq!(seqs(&page), [2, 3]);
        assert_eq!(
            (page.next, page.more),
            (4, false),
            "d4 was scanned and skipped"
        );
        let errors = b.query(&LogQuery {
            min_level: Level::Error,
            ..LogQuery::default()
        });
        assert!(errors.entries.is_empty());
        assert_eq!(errors.next, 4);
    }

    #[test]
    fn limit_pages_through_the_buffer() {
        let b = LogBuffer::new(10);
        for i in 0..5 {
            b.push(Level::Info, "t", &format!("m{i}"), "");
        }
        let first = b.query(&LogQuery {
            limit: 2,
            ..LogQuery::default()
        });
        assert_eq!(seqs(&first), [1, 2]);
        assert_eq!((first.next, first.more), (2, true));
        let second = b.query(&LogQuery {
            after: first.next,
            limit: 2,
            ..LogQuery::default()
        });
        assert_eq!(seqs(&second), [3, 4]);
        let last = b.query(&LogQuery {
            after: second.next,
            limit: 2,
            ..LogQuery::default()
        });
        assert_eq!(seqs(&last), [5]);
        assert_eq!((last.next, last.more), (5, false));
    }

    #[test]
    fn evicted_lines_are_counted_as_dropped() {
        let b = LogBuffer::new(3);
        for i in 0..5 {
            b.push(Level::Info, "t", &format!("m{i}"), "");
        }
        let page = b.query(&LogQuery::default());
        assert_eq!(seqs(&page), [3, 4, 5]);
        assert_eq!(page.dropped, 2);
        let page = b.query(&LogQuery {
            after: 1,
            ..LogQuery::default()
        });
        assert_eq!((seqs(&page), page.dropped), (vec![3, 4, 5], 1));
        let page = b.query(&LogQuery {
            after: 2,
            ..LogQuery::default()
        });
        assert_eq!(page.dropped, 0);
    }

    #[test]
    fn a_cursor_beyond_the_newest_line_returns_nothing() {
        let b = LogBuffer::new(3);
        b.push(Level::Info, "t", "m", "");
        let page = b.query(&LogQuery {
            after: 99,
            ..LogQuery::default()
        });
        assert!(page.entries.is_empty());
        assert_eq!((page.next, page.more, page.dropped), (99, false, 0));
    }

    #[test]
    fn capacity_zero_keeps_nothing_and_capacity_is_capped() {
        let off = LogBuffer::new(0);
        assert_eq!(off.push(Level::Error, "t", "m", ""), 0);
        let page = off.query(&LogQuery::default());
        assert!(page.entries.is_empty());
        assert_eq!((page.next, page.dropped), (0, 0));
        assert!(off.boot() > 0);
        assert_eq!(LogBuffer::new(MAX_CAPACITY + 1).capacity(), MAX_CAPACITY);
    }

    #[test]
    fn push_scrubs_and_caps_what_it_stores() {
        let b = LogBuffer::new(3);
        b.push(
            Level::Warn,
            "t",
            &format!("Bearer abc {}", "x".repeat(5000)),
            "api_key=k1 room_id=R1",
        );
        let page = b.query(&LogQuery::default());
        let e = &page.entries[0];
        assert!(
            e.message.starts_with("Bearer <redacted> "),
            "{}",
            &e.message[..40]
        );
        assert!(e.message.len() <= MAX_MESSAGE_BYTES + '…'.len_utf8());
        assert_eq!(e.fields, "api_key=<redacted> room_id=R1");
    }

    #[test]
    fn concurrent_pushes_keep_seqs_unique_and_contiguous() {
        let b = Arc::new(LogBuffer::new(MAX_CAPACITY));
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let b = b.clone();
                std::thread::spawn(move || {
                    for i in 0..2000 {
                        b.push(Level::Info, "t", &format!("{t}-{i}"), "");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut after = 0;
        let mut seen = Vec::new();
        loop {
            let page = b.query(&LogQuery {
                after,
                limit: MAX_LIMIT,
                ..LogQuery::default()
            });
            seen.extend(seqs(&page));
            after = page.next;
            if !page.more {
                break;
            }
        }
        // 16000 pushed, the newest 10000 kept, in order and without a hole.
        assert_eq!(seen.len(), MAX_CAPACITY);
        assert_eq!(seen.first(), Some(&6001));
        assert!(seen.windows(2).all(|w| w[1] == w[0] + 1));
    }

    #[test]
    fn query_parameters_are_read_and_checked() {
        assert_eq!(LogQuery::from_pairs(&[]).unwrap(), LogQuery::default());
        assert_eq!(
            LogQuery::from_pairs(&pairs(&[
                ("after", "41"),
                ("level", "WARN"),
                ("limit", "1000")
            ]))
            .unwrap(),
            LogQuery {
                after: 41,
                min_level: Level::Warn,
                limit: 1000
            }
        );
        for (list, needle) in [
            (vec![("after", "abc")], "after must be a whole number"),
            (vec![("after", "-1")], "after must be a whole number"),
            (vec![("after", "+1")], "after must be a whole number"),
            (vec![("after", " 1")], "after must be a whole number"),
            (vec![("after", "")], "after must be a whole number"),
            (
                vec![("level", "loud")],
                "level must be one of error, warn, info, debug, trace",
            ),
            (
                vec![("limit", "0")],
                "limit must be a whole number from 1 to 1000",
            ),
            (
                vec![("limit", "1001")],
                "limit must be a whole number from 1 to 1000",
            ),
            (vec![("limit", "ten")], "got 'ten'"),
            (
                vec![("since", "1")],
                "Unknown query parameter 'since'. Valid parameters: after, level, limit.",
            ),
            (
                vec![("after", "1"), ("after", "2")],
                "'after' is given twice",
            ),
        ] {
            let err = LogQuery::from_pairs(&pairs(&list)).unwrap_err();
            assert!(err.contains(needle), "{list:?}: {err}");
        }
        let long = "x".repeat(500);
        let err = LogQuery::from_pairs(&pairs(&[("level", long.as_str())])).unwrap_err();
        assert!(err.len() < 200, "the value is shortened: {err}");
    }
}

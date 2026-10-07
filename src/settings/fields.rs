//! Catalogue of the variables the panel can edit, with their scope and code default.
//!
//! A drift test checks that every variable the config parsers read is listed here or in
//! `BOOT_ONLY`, and that each default below equals the one the code applies.

use super::store::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    Choice(&'static [&'static str]),
    List,
    Secret,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Text => "text",
            Kind::Choice(_) => "choice",
            Kind::List => "list",
            Kind::Secret => "secret",
        }
    }

    pub fn choices(self) -> Option<&'static [&'static str]> {
        match self {
            Kind::Choice(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub scope: Scope,
    pub kind: Kind,
    /// The value the code applies when nothing is set (empty = unset).
    pub default: &'static str,
}

const fn f(name: &'static str, scope: Scope, kind: Kind, default: &'static str) -> Field {
    Field {
        name,
        scope,
        kind,
        default,
    }
}

use Kind::*;
use Scope::{FlickDd as Dd, FlickSync as Sy, Server as Srv};

pub const FIELDS: &[Field] = &[
    // --- server
    f("FLICKSYNC_PUBLIC_URL", Srv, Text, ""),
    f("FLICKSYNC_AUTH_KEYS", Srv, Secret, ""),
    f("FLICKSYNC_AUTH_AUDIENCE", Srv, Text, "flicksync"),
    f("FLICKSYNC_AUTH_MAX_TOKEN_TTL", Srv, Int, "86400"),
    f("FLICKSYNC_AUTH_LEEWAY", Srv, Int, "30"),
    f("FLICKSYNC_CORS_ORIGINS", Srv, List, ""),
    f("FLICKSYNC_METRICS_ENABLED", Srv, Bool, "false"),
    f("FLICKSYNC_METRICS_TOKEN", Srv, Secret, ""),
    f("FLICKSYNC_LOG_LEVEL", Srv, Text, "info"),
    // --- FlickSync
    f("FLICKSYNC_ENABLED", Sy, Bool, "false"),
    f("FLICKSYNC_MAX_ROOM_SIZE", Sy, Int, "100"),
    f("FLICKSYNC_MAX_ROOMS", Sy, Int, "10000"),
    f("FLICKSYNC_ROOM_CREATE_PER_MINUTE", Sy, Int, "6"),
    f("FLICKSYNC_ROOM_TIMEOUT", Sy, Int, "60"),
    f("FLICKSYNC_ROOM_IDLE_TIMEOUT", Sy, Int, "43200"),
    f("FLICKSYNC_RECONNECT_GRACE", Sy, Int, "30"),
    f("FLICKSYNC_CONNECT_GRACE", Sy, Int, "60"),
    f(
        "FLICKSYNC_HOST_LEAVE_POLICY",
        Sy,
        Choice(&["transfer", "close"]),
        "transfer",
    ),
    f(
        "FLICKSYNC_DEFAULT_CONTROL_MODE",
        Sy,
        Choice(&["everyone", "host_only"]),
        "everyone",
    ),
    f("FLICKSYNC_SYNC_DRIFT_IGNORE", Sy, Float, "100"),
    f("FLICKSYNC_SYNC_DRIFT_SOFT", Sy, Float, "500"),
    f("FLICKSYNC_SYNC_DRIFT_HARD", Sy, Float, "1500"),
    f("FLICKSYNC_SYNC_RATE_SOFT", Sy, Float, "0.03"),
    f("FLICKSYNC_SYNC_RATE_STRONG", Sy, Float, "0.08"),
    f("FLICKSYNC_SYNC_SEEK_COOLDOWN_MS", Sy, Int, "3000"),
    f("FLICKSYNC_SYNC_HEARTBEAT", Sy, Int, "10"),
    f("FLICKSYNC_RATE_MIN", Sy, Float, "0.25"),
    f("FLICKSYNC_RATE_MAX", Sy, Float, "4.0"),
    f("FLICKSYNC_MAX_POSITION", Sy, Int, "604800"),
    f("FLICKSYNC_CHAT_ENABLED", Sy, Bool, "true"),
    f("FLICKSYNC_CHAT_MAX_LENGTH", Sy, Int, "500"),
    f("FLICKSYNC_CHAT_HISTORY", Sy, Int, "100"),
    f("FLICKSYNC_CHAT_RATE_PER_SEC", Sy, Float, "1"),
    f("FLICKSYNC_CHAT_BURST", Sy, Int, "5"),
    f("FLICKSYNC_MAX_CONNECTIONS", Sy, Int, "10000"),
    f("FLICKSYNC_WS_MAX_MESSAGE_BYTES", Sy, Int, "16384"),
    f("FLICKSYNC_WS_PING_INTERVAL", Sy, Int, "20"),
    f("FLICKSYNC_WS_IDLE_TIMEOUT", Sy, Int, "60"),
    f("FLICKSYNC_WS_SEND_TIMEOUT", Sy, Int, "10"),
    f("FLICKSYNC_MSG_RATE_PER_SEC", Sy, Int, "20"),
    f("FLICKSYNC_MSG_BURST", Sy, Int, "40"),
    f("FLICKSYNC_WS_RATE_LIMIT_STRIKES", Sy, Int, "20"),
    f("FLICKSYNC_WS_OUTBOUND_BUFFER", Sy, Int, "256"),
    // --- FlickDD
    f("FLICKDD_ENABLED", Dd, Bool, "false"),
    f("FLICKDD_JELLYFIN_URL", Dd, Text, ""),
    f("FLICKDD_JELLYFIN_API_KEY", Dd, Secret, ""),
    f("FLICKDD_PLEX_URL", Dd, Text, ""),
    f("FLICKDD_PLEX_TOKEN", Dd, Secret, ""),
    f("FLICKDD_MAX_PARALLEL", Dd, Int, "10"),
    f("FLICKDD_MAX_GLOBAL", Dd, Int, "100"),
    f("FLICKDD_RATE_MBPS", Dd, Int, "10"),
    f("FLICKDD_CHUNK_MB", Dd, Int, "8"),
    f("FLICKDD_MAX_RANGE_MB", Dd, Int, "64"),
    f("FLICKDD_MAX_REQUESTS_PER_MIN", Dd, Int, "120"),
    f("FLICKDD_MAX_OVERSERVE", Dd, Int, "2"),
    f("FLICKDD_GRANT_TTL", Dd, Int, "21600"),
    f("FLICKDD_GRANT_MAX_AGE", Dd, Int, "86400"),
    f("FLICKDD_STALL_TIMEOUT", Dd, Int, "30"),
    f("FLICKDD_UPSTREAM_TIMEOUT", Dd, Int, "15"),
    f("FLICKDD_UPSTREAM_RETRIES", Dd, Int, "2"),
];

/// Read by the config but fixed at process start (or bootstrap secrets): never editable.
pub const BOOT_ONLY: &[&str] = &[
    "FLICKSYNC_HOST",
    "FLICKSYNC_PORT",
    "FLICKSYNC_DATA_DIR",
    "FLICKSYNC_LOG_FORMAT",
    "FLICKSYNC_AUTH_KEYS_FILE",
    "FLICKSYNC_SHUTDOWN_GRACE",
    "FLICKSYNC_SWEEP_INTERVAL_MS",
    "FLICKSYNC_MAX_BODY_BYTES",
    "FLICKSYNC_ADMIN_TOKEN",
    "PANEL_PASSWORD",
];

pub fn field(name: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::Config;

    fn quoted_names(src: &str) -> Vec<&str> {
        src.split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| {
                (s.starts_with("FLICKSYNC_") || s.starts_with("FLICKDD_") || *s == "PANEL_PASSWORD")
                    && s.bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            })
            .collect()
    }

    #[test]
    fn every_variable_the_config_reads_is_catalogued_or_boot_only() {
        for src in [
            include_str!("../config/mod.rs"),
            include_str!("../dd/config.rs"),
        ] {
            for name in quoted_names(src) {
                assert!(
                    field(name).is_some() || BOOT_ONLY.contains(&name),
                    "{name} is read by the config but is neither in FIELDS nor in BOOT_ONLY"
                );
            }
        }
    }

    #[test]
    fn a_name_is_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for f in FIELDS {
            assert!(seen.insert(f.name), "{} is listed twice", f.name);
            assert!(
                !BOOT_ONLY.contains(&f.name),
                "{} is both editable and boot-only",
                f.name
            );
        }
    }

    #[test]
    fn catalogue_defaults_match_the_code_defaults() {
        let base = [
            ("FLICKDD_ENABLED", "true"),
            ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
            ("FLICKDD_JELLYFIN_API_KEY", "k"),
        ];
        let build = |extra: Option<(&str, &str)>| {
            let mut m: HashMap<String, String> = base
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            if let Some((k, v)) = extra {
                m.insert(k.into(), v.into());
            }
            format!(
                "{:?}",
                Config::from_lookup(&move |k| m.get(k).cloned()).unwrap()
            )
        };
        let reference = build(None);
        for f in FIELDS.iter().filter(|f| {
            f.kind != Kind::Secret
                && !f.default.is_empty()
                && !base.iter().any(|(k, _)| *k == f.name)
        }) {
            assert_eq!(build(Some((f.name, f.default))), reference, "{}", f.name);
        }
    }
}

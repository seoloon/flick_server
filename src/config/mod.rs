//! Configuration from environment variables (see `.env.example`).
//!
//! Nothing operational is hard-coded: every limit and threshold below has a
//! default and an environment override. Parsing goes through a lookup function
//! so tests do not need to touch the process environment.

use std::str::FromStr;
use std::sync::Arc;

use crate::auth::AuthConfig;
use crate::chat::ChatConfig;
use crate::invite::Endpoint;
use crate::protocol::ControlMode;
use crate::room::manager::ManagerConfig;
use crate::room::{HostLeavePolicy, RoomConfig};
use crate::sync::DriftConfig;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid value for {name}: '{value}' ({reason})")]
    Invalid {
        name: String,
        value: String,
        reason: String,
    },
    #[error("{0}")]
    Inconsistent(String),
    #[error("cannot read {path}: {source}")]
    File {
        path: String,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone)]
pub struct WsConfig {
    /// Hard cap on a single inbound message/frame.
    pub max_message_bytes: usize,
    /// Connection is dropped when nothing (including pongs) is received for this long.
    pub idle_timeout_secs: u64,
    /// Interval of server-initiated WebSocket ping frames.
    pub ping_interval_secs: u64,
    /// A single outbound write may not block longer than this.
    pub send_timeout_secs: u64,
    /// Close the connection after this many consecutive rate-limit violations.
    pub rate_limit_strikes: u32,
    /// Global cap on simultaneous WebSocket connections.
    pub max_connections: usize,
}

#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Allowed browser origins (CORS and WebSocket `Origin`). Empty = browsers denied.
    pub allowed_origins: Vec<String>,
    pub metrics_enabled: bool,
    pub metrics_token: Option<String>,
    /// Bearer token of the admin API (`/admin/v1/*`); `None` = admin API disabled (404).
    pub admin_token: Option<String>,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub log_format: LogFormat,
    pub manager: ManagerConfig,
    pub auth: AuthConfig,
    pub ws: WsConfig,
    pub http: HttpConfig,
    pub sweep_interval_ms: u64,
    pub shutdown_grace_secs: u64,
    /// FlickSync starts at boot (FLICKSYNC_ENABLED, default false).
    pub sync_enabled: bool,
    /// Where the auto-generated signing key lives (`FLICKSYNC_DATA_DIR`).
    pub data_dir: String,
    /// True when `FLICKSYNC_AUTH_KEYS` or `FLICKSYNC_AUTH_KEYS_FILE` is set: no key is generated then.
    pub keys_configured: bool,
    /// Public address used in invitations (`FLICKSYNC_PUBLIC_URL`).
    pub public: Option<Endpoint>,
    /// FlickDD streaming-download module (`FLICKDD_*`).
    pub dd: crate::dd::config::DdConfig,
}

pub(crate) type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

pub(crate) fn parse<T>(env: Lookup, name: &str, default: T) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match env(name)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
    {
        None => Ok(default),
        Some(v) => v.parse::<T>().map_err(|e| ConfigError::Invalid {
            name: name.to_owned(),
            value: v,
            reason: e.to_string(),
        }),
    }
}

pub(crate) fn parse_bool(env: Lookup, name: &str, default: bool) -> Result<bool, ConfigError> {
    match env(name)
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
    {
        None => Ok(default),
        Some(v) => match v.as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(ConfigError::Invalid {
                name: name.to_owned(),
                value: v,
                reason: "expected true/false".into(),
            }),
        },
    }
}

fn parse_list(env: Lookup, name: &str) -> Vec<String> {
    env(name)
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().trim_end_matches('/').to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn secs_to_ms(s: u64) -> u64 {
    s.saturating_mul(1000)
}

fn inconsistent(msg: &str) -> ConfigError {
    ConfigError::Inconsistent(msg.to_owned())
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(&|k| std::env::var(k).ok())
    }

    pub fn from_lookup(env: Lookup) -> Result<Self, ConfigError> {
        let d_room = RoomConfig::default();
        let d_drift = DriftConfig::default();
        let d_chat = ChatConfig::default();
        let d_mgr = ManagerConfig::default();
        let d_auth = AuthConfig::default();

        let host_leave_policy =
            match parse::<String>(env, "FLICKSYNC_HOST_LEAVE_POLICY", "transfer".into())?
                .to_ascii_lowercase()
                .as_str()
            {
                "transfer" => HostLeavePolicy::Transfer,
                "close" => HostLeavePolicy::Close,
                other => {
                    return Err(ConfigError::Invalid {
                        name: "FLICKSYNC_HOST_LEAVE_POLICY".into(),
                        value: other.into(),
                        reason: "expected 'transfer' or 'close'".into(),
                    });
                }
            };
        let default_control_mode =
            match parse::<String>(env, "FLICKSYNC_DEFAULT_CONTROL_MODE", "everyone".into())?
                .to_ascii_lowercase()
                .as_str()
            {
                "everyone" => ControlMode::Everyone,
                "host_only" => ControlMode::HostOnly,
                other => {
                    return Err(ConfigError::Invalid {
                        name: "FLICKSYNC_DEFAULT_CONTROL_MODE".into(),
                        value: other.into(),
                        reason: "expected 'everyone' or 'host_only'".into(),
                    });
                }
            };

        let drift = DriftConfig {
            ignore_ms: parse(env, "FLICKSYNC_SYNC_DRIFT_IGNORE", d_drift.ignore_ms)?,
            soft_ms: parse(env, "FLICKSYNC_SYNC_DRIFT_SOFT", d_drift.soft_ms)?,
            hard_ms: parse(env, "FLICKSYNC_SYNC_DRIFT_HARD", d_drift.hard_ms)?,
            rate_soft: parse(env, "FLICKSYNC_SYNC_RATE_SOFT", d_drift.rate_soft)?,
            rate_strong: parse(env, "FLICKSYNC_SYNC_RATE_STRONG", d_drift.rate_strong)?,
            seek_cooldown_ms: parse(
                env,
                "FLICKSYNC_SYNC_SEEK_COOLDOWN_MS",
                d_drift.seek_cooldown_ms,
            )?,
        };
        if !(0.0 < drift.ignore_ms
            && drift.ignore_ms < drift.soft_ms
            && drift.soft_ms < drift.hard_ms)
        {
            return Err(inconsistent(
                "sync thresholds must satisfy 0 < IGNORE < SOFT < HARD (milliseconds)",
            ));
        }
        if !(0.0 < drift.rate_soft
            && drift.rate_soft <= drift.rate_strong
            && drift.rate_strong < 0.5)
        {
            return Err(inconsistent(
                "sync rate factors must satisfy 0 < RATE_SOFT <= RATE_STRONG < 0.5",
            ));
        }

        let chat = ChatConfig {
            max_message_chars: parse(env, "FLICKSYNC_CHAT_MAX_LENGTH", d_chat.max_message_chars)?,
            max_history: parse(env, "FLICKSYNC_CHAT_HISTORY", d_chat.max_history)?,
            rate_per_sec: parse(env, "FLICKSYNC_CHAT_RATE_PER_SEC", d_chat.rate_per_sec)?,
            burst: parse(env, "FLICKSYNC_CHAT_BURST", d_chat.burst)?,
        };

        let room = RoomConfig {
            max_participants: parse(env, "FLICKSYNC_MAX_ROOM_SIZE", d_room.max_participants)?,
            host_leave_policy,
            default_control_mode,
            chat_enabled: parse_bool(env, "FLICKSYNC_CHAT_ENABLED", d_room.chat_enabled)?,
            reconnect_grace_ms: secs_to_ms(parse(
                env,
                "FLICKSYNC_RECONNECT_GRACE",
                d_room.reconnect_grace_ms / 1000,
            )?),
            connect_grace_ms: secs_to_ms(parse(
                env,
                "FLICKSYNC_CONNECT_GRACE",
                d_room.connect_grace_ms / 1000,
            )?),
            empty_timeout_ms: secs_to_ms(parse(
                env,
                "FLICKSYNC_ROOM_TIMEOUT",
                d_room.empty_timeout_ms / 1000,
            )?),
            idle_timeout_ms: secs_to_ms(parse(
                env,
                "FLICKSYNC_ROOM_IDLE_TIMEOUT",
                d_room.idle_timeout_ms / 1000,
            )?),
            heartbeat_interval_ms: secs_to_ms(parse(
                env,
                "FLICKSYNC_SYNC_HEARTBEAT",
                d_room.heartbeat_interval_ms / 1000,
            )?),
            rate_min: parse(env, "FLICKSYNC_RATE_MIN", d_room.rate_min)?,
            rate_max: parse(env, "FLICKSYNC_RATE_MAX", d_room.rate_max)?,
            max_position_secs: parse(env, "FLICKSYNC_MAX_POSITION", d_room.max_position_secs)?,
            msg_rate_per_sec: parse(env, "FLICKSYNC_MSG_RATE_PER_SEC", d_room.msg_rate_per_sec)?,
            msg_burst: parse(env, "FLICKSYNC_MSG_BURST", d_room.msg_burst)?,
            drift,
            chat,
        };
        if room.max_participants == 0 {
            return Err(inconsistent("FLICKSYNC_MAX_ROOM_SIZE must be at least 1"));
        }
        if !(room.rate_min > 0.0 && room.rate_min <= 1.0 && room.rate_max >= 1.0) {
            return Err(inconsistent(
                "FLICKSYNC_RATE_MIN must be in (0, 1] and FLICKSYNC_RATE_MAX must be >= 1",
            ));
        }
        if room.heartbeat_interval_ms == 0 {
            return Err(inconsistent(
                "FLICKSYNC_SYNC_HEARTBEAT must be at least 1 second",
            ));
        }

        let manager = ManagerConfig {
            room: Arc::new(room),
            max_rooms: parse(env, "FLICKSYNC_MAX_ROOMS", d_mgr.max_rooms)?,
            create_per_minute: parse(
                env,
                "FLICKSYNC_ROOM_CREATE_PER_MINUTE",
                d_mgr.create_per_minute,
            )?,
            outbound_buffer: parse(env, "FLICKSYNC_WS_OUTBOUND_BUFFER", d_mgr.outbound_buffer)?,
        };

        let mut keys = parse_key_list(&env("FLICKSYNC_AUTH_KEYS").unwrap_or_default());
        let keys_file = env("FLICKSYNC_AUTH_KEYS_FILE").filter(|p| !p.trim().is_empty());
        let keys_configured = !keys.is_empty() || keys_file.is_some();
        if let Some(path) = keys_file {
            let content =
                std::fs::read_to_string(path.trim()).map_err(|source| ConfigError::File {
                    path: path.trim().to_owned(),
                    source,
                })?;
            keys.extend(parse_key_list(&content));
        }
        let auth = AuthConfig {
            keys,
            audience: parse(env, "FLICKSYNC_AUTH_AUDIENCE", d_auth.audience)?,
            max_token_ttl_secs: parse(
                env,
                "FLICKSYNC_AUTH_MAX_TOKEN_TTL",
                d_auth.max_token_ttl_secs,
            )?,
            leeway_secs: parse(env, "FLICKSYNC_AUTH_LEEWAY", d_auth.leeway_secs)?,
        };

        let ws = WsConfig {
            max_message_bytes: parse(env, "FLICKSYNC_WS_MAX_MESSAGE_BYTES", 16 * 1024)?,
            idle_timeout_secs: parse(env, "FLICKSYNC_WS_IDLE_TIMEOUT", 60)?,
            ping_interval_secs: parse(env, "FLICKSYNC_WS_PING_INTERVAL", 20)?,
            send_timeout_secs: parse(env, "FLICKSYNC_WS_SEND_TIMEOUT", 10)?,
            rate_limit_strikes: parse(env, "FLICKSYNC_WS_RATE_LIMIT_STRIKES", 20)?,
            max_connections: parse(env, "FLICKSYNC_MAX_CONNECTIONS", 10_000)?,
        };
        if ws.ping_interval_secs == 0 || ws.idle_timeout_secs <= ws.ping_interval_secs {
            return Err(inconsistent(
                "FLICKSYNC_WS_IDLE_TIMEOUT must be greater than FLICKSYNC_WS_PING_INTERVAL (and the latter > 0)",
            ));
        }

        let http = HttpConfig {
            allowed_origins: parse_list(env, "FLICKSYNC_CORS_ORIGINS"),
            metrics_enabled: parse_bool(env, "FLICKSYNC_METRICS_ENABLED", false)?,
            metrics_token: env("FLICKSYNC_METRICS_TOKEN").filter(|t| !t.trim().is_empty()),
            admin_token: env("FLICKSYNC_ADMIN_TOKEN")
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty()),
            max_body_bytes: parse(env, "FLICKSYNC_MAX_BODY_BYTES", 16 * 1024)?,
        };
        if http.admin_token.as_ref().is_some_and(|t| t.len() < 16) {
            return Err(inconsistent(
                "FLICKSYNC_ADMIN_TOKEN must be at least 16 characters (e.g. `openssl rand -base64 32`)",
            ));
        }
        if http.allowed_origins.iter().any(|o| o == "*") {
            return Err(inconsistent(
                "FLICKSYNC_CORS_ORIGINS must list explicit origins; '*' is not allowed because requests are authenticated",
            ));
        }

        let log_format = match parse::<String>(env, "FLICKSYNC_LOG_FORMAT", "pretty".into())?
            .to_ascii_lowercase()
            .as_str()
        {
            "pretty" | "text" => LogFormat::Pretty,
            "json" => LogFormat::Json,
            other => {
                return Err(ConfigError::Invalid {
                    name: "FLICKSYNC_LOG_FORMAT".into(),
                    value: other.into(),
                    reason: "expected 'pretty' or 'json'".into(),
                });
            }
        };

        let public = match env("FLICKSYNC_PUBLIC_URL").filter(|v| !v.trim().is_empty()) {
            None => None,
            Some(v) => Some(
                Endpoint::from_public_url(&v).map_err(|e| ConfigError::Invalid {
                    name: "FLICKSYNC_PUBLIC_URL".into(),
                    value: v.trim().to_owned(),
                    reason: e.to_string(),
                })?,
            ),
        };

        Ok(Config {
            host: parse(env, "FLICKSYNC_HOST", "0.0.0.0".to_owned())?,
            port: parse(env, "FLICKSYNC_PORT", 8787)?,
            log_level: parse(env, "FLICKSYNC_LOG_LEVEL", "info".to_owned())?,
            log_format,
            manager,
            auth,
            ws,
            http,
            sweep_interval_ms: parse(env, "FLICKSYNC_SWEEP_INTERVAL_MS", 1000)?,
            shutdown_grace_secs: parse(env, "FLICKSYNC_SHUTDOWN_GRACE", 10)?,
            sync_enabled: parse_bool(env, "FLICKSYNC_ENABLED", false)?,
            data_dir: parse(env, "FLICKSYNC_DATA_DIR", "./data".to_owned())?,
            keys_configured,
            public,
            // Only `FLICKDD_ENABLED` is read while FlickDD is off: a stray FLICKDD_* value can
            // never keep FlickSync from starting.
            dd: crate::dd::config::DdConfig::from_lookup(env)?,
        })
    }
}

fn parse_key_list(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn flicksync_is_disabled_unless_enabled_explicitly() {
        assert!(!cfg(&[]).unwrap().sync_enabled);
        assert!(cfg(&[("FLICKSYNC_ENABLED", "true")]).unwrap().sync_enabled);
        assert!(cfg(&[("FLICKSYNC_ENABLED", "perhaps")]).is_err());
    }
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(&move |k| m.get(k).cloned())
    }

    #[test]
    fn defaults_are_sane() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.port, 8787);
        assert_eq!(c.manager.room.drift.hard_ms, 1500.0);
        assert!(c.http.allowed_origins.is_empty());
        assert!(!c.http.metrics_enabled);
        assert_eq!(c.manager.room.host_leave_policy, HostLeavePolicy::Transfer);
    }

    #[test]
    fn overrides_apply() {
        let c = cfg(&[
            ("FLICKSYNC_PORT", "9000"),
            ("FLICKSYNC_MAX_ROOM_SIZE", "5"),
            ("FLICKSYNC_ROOM_TIMEOUT", "5"),
            ("FLICKSYNC_SYNC_DRIFT_SOFT", "300"),
            ("FLICKSYNC_HOST_LEAVE_POLICY", "close"),
            ("FLICKSYNC_CORS_ORIGINS", "https://a.example, https://b.example/"),
            ("FLICKSYNC_AUTH_KEYS", "k1:srv:secretsecretsecretsecretsecretsecret, k2:srv:secretsecretsecretsecretsecretsecr2"),
        ])
        .unwrap();
        assert_eq!(c.port, 9000);
        assert_eq!(c.manager.room.max_participants, 5);
        assert_eq!(c.manager.room.empty_timeout_ms, 5000);
        assert_eq!(c.manager.room.drift.soft_ms, 300.0);
        assert_eq!(c.manager.room.host_leave_policy, HostLeavePolicy::Close);
        assert_eq!(
            c.http.allowed_origins,
            vec!["https://a.example", "https://b.example"]
        );
        assert_eq!(c.auth.keys.len(), 2);
    }

    #[test]
    fn invalid_values_are_rejected_with_the_variable_name() {
        let e = cfg(&[("FLICKSYNC_PORT", "nope")]).unwrap_err().to_string();
        assert!(e.contains("FLICKSYNC_PORT"));
        assert!(cfg(&[("FLICKSYNC_HOST_LEAVE_POLICY", "explode")]).is_err());
        assert!(cfg(&[("FLICKSYNC_MAX_ROOM_SIZE", "0")]).is_err());
    }

    #[test]
    fn inconsistent_thresholds_are_rejected() {
        assert!(cfg(&[("FLICKSYNC_SYNC_DRIFT_SOFT", "50")]).is_err());
        assert!(cfg(&[("FLICKSYNC_SYNC_DRIFT_HARD", "400")]).is_err());
        assert!(cfg(&[("FLICKSYNC_WS_IDLE_TIMEOUT", "10")]).is_err());
    }

    #[test]
    fn key_sources_and_public_url() {
        let c = cfg(&[]).unwrap();
        assert!(!c.keys_configured);
        assert_eq!(c.data_dir, "./data");
        assert!(c.public.is_none());
        let c = cfg(&[("FLICKSYNC_AUTH_KEYS", "k:s:x")]).unwrap();
        assert!(c.keys_configured);
        let c = cfg(&[
            ("FLICKSYNC_PUBLIC_URL", "https://sync.example.com/"),
            ("FLICKSYNC_DATA_DIR", "/data"),
        ])
        .unwrap();
        let p = c.public.unwrap();
        assert!(p.tls);
        assert_eq!(p.authority, "sync.example.com");
        assert_eq!(p.path, "");
        assert_eq!(c.data_dir, "/data");
        let e = cfg(&[("FLICKSYNC_PUBLIC_URL", "ftp://x")]).unwrap_err();
        assert!(e.to_string().contains("FLICKSYNC_PUBLIC_URL"));
    }

    #[test]
    fn admin_token_is_optional_but_not_weak() {
        assert!(cfg(&[]).unwrap().http.admin_token.is_none());
        assert!(
            cfg(&[("FLICKSYNC_ADMIN_TOKEN", "  ")])
                .unwrap()
                .http
                .admin_token
                .is_none()
        );
        assert!(cfg(&[("FLICKSYNC_ADMIN_TOKEN", "short")]).is_err());
        let c = cfg(&[("FLICKSYNC_ADMIN_TOKEN", "0123456789abcdef0123")]).unwrap();
        assert_eq!(c.http.admin_token.as_deref(), Some("0123456789abcdef0123"));
    }

    #[test]
    fn flickdd_variables_only_matter_when_flickdd_is_enabled() {
        let broken = [
            ("FLICKDD_PLEX_URL", "http://plex:32400"),
            ("FLICKDD_MAX_PARALLEL", "0"),
            ("FLICKDD_CHUNK_MB", "nope"),
        ];
        let c = cfg(&broken).unwrap();
        assert!(!c.dd.enabled);
        let mut off = broken.to_vec();
        off.push(("FLICKDD_ENABLED", "false"));
        assert!(cfg(&off).is_ok());
        let mut on = broken.to_vec();
        on.push(("FLICKDD_ENABLED", "true"));
        assert!(cfg(&on).is_err());
    }

    #[test]
    fn wildcard_cors_is_refused() {
        assert!(cfg(&[("FLICKSYNC_CORS_ORIGINS", "*")]).is_err());
    }

    #[test]
    fn key_list_parsing_handles_newlines_and_comments() {
        let k = parse_key_list("# comment\nk1:s:abc\n\nk2:s:def, k3:s:ghi");
        assert_eq!(k, vec!["k1:s:abc", "k2:s:def", "k3:s:ghi"]);
    }
}

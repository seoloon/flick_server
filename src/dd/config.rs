//! FlickDD configuration (`FLICKDD_*` environment variables).

use crate::config::{ConfigError, Lookup, parse, parse_bool};

#[derive(Clone)]
pub struct BackendConfig {
    pub url: String,
    pub secret: String,
}

/// Manual `Debug`: the secret must never reach logs (`DdConfig` derives `Debug`).
impl std::fmt::Debug for BackendConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendConfig")
            .field("url", &self.url)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct DdConfig {
    pub enabled: bool,
    pub jellyfin: Option<BackendConfig>,
    pub plex: Option<BackendConfig>,
    pub max_parallel: usize,
    pub max_global: usize,
    pub rate_bps: u64,
    pub chunk_bytes: u64,
    pub max_range_bytes: u64,
    pub grant_ttl_ms: u64,
    pub grant_max_age_ms: u64,
    pub stall_timeout_ms: u64,
    pub upstream_timeout_ms: u64,
    pub upstream_retries: u32,
    pub max_requests_per_min: u32,
    pub max_overserve: u64,
}

fn backend(
    env: Lookup,
    label: &str,
    url_var: &str,
    secret_var: &str,
) -> Result<Option<BackendConfig>, ConfigError> {
    let get = |n: &str| {
        env(n)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    match (get(url_var), get(secret_var)) {
        (None, None) => Ok(None),
        (Some(url), Some(secret)) => Ok(Some(BackendConfig {
            url: url.trim_end_matches('/').to_owned(),
            secret,
        })),
        _ => Err(ConfigError::Inconsistent(format!(
            "the {label} backend is half configured: set both {url_var} and {secret_var}, or clear both"
        ))),
    }
}

impl DdConfig {
    /// `FLICKDD_ENABLED` (default false) decides everything else: FlickDD is an optional
    /// module and must never keep FlickSync from starting. With the flag off, every other
    /// `FLICKDD_*` variable is ignored (neither parsed nor validated) and the defaults apply,
    /// without any backend. With the flag on, they are all parsed and validated.
    pub fn from_lookup(env: Lookup) -> Result<Self, ConfigError> {
        if !parse_bool(env, "FLICKDD_ENABLED", false)? {
            return Self::read(&|_| None, false);
        }
        let c = Self::read(env, true)?;
        c.validate()?;
        Ok(c)
    }

    /// Every `FLICKDD_*` value but the flag, with its default.
    fn read(env: Lookup, enabled: bool) -> Result<Self, ConfigError> {
        const MIB: u64 = 1024 * 1024;
        let jellyfin = backend(
            env,
            "Jellyfin",
            "FLICKDD_JELLYFIN_URL",
            "FLICKDD_JELLYFIN_API_KEY",
        )?;
        let plex = backend(env, "Plex", "FLICKDD_PLEX_URL", "FLICKDD_PLEX_TOKEN")?;
        Ok(Self {
            enabled,
            jellyfin,
            plex,
            max_parallel: parse(env, "FLICKDD_MAX_PARALLEL", 10usize)?,
            max_global: parse(env, "FLICKDD_MAX_GLOBAL", 100usize)?,
            rate_bps: parse(env, "FLICKDD_RATE_MBPS", 10u64)?.saturating_mul(MIB),
            chunk_bytes: parse(env, "FLICKDD_CHUNK_MB", 8u64)?.saturating_mul(MIB),
            max_range_bytes: parse(env, "FLICKDD_MAX_RANGE_MB", 64u64)?.saturating_mul(MIB),
            grant_ttl_ms: parse(env, "FLICKDD_GRANT_TTL", 21_600u64)?.saturating_mul(1000),
            grant_max_age_ms: parse(env, "FLICKDD_GRANT_MAX_AGE", 86_400u64)?.saturating_mul(1000),
            stall_timeout_ms: parse(env, "FLICKDD_STALL_TIMEOUT", 30u64)?.saturating_mul(1000),
            upstream_timeout_ms: parse(env, "FLICKDD_UPSTREAM_TIMEOUT", 15u64)?
                .saturating_mul(1000),
            upstream_retries: parse(env, "FLICKDD_UPSTREAM_RETRIES", 2u32)?,
            max_requests_per_min: parse(env, "FLICKDD_MAX_REQUESTS_PER_MIN", 120u32)?,
            max_overserve: parse(env, "FLICKDD_MAX_OVERSERVE", 2u64)?,
        })
    }

    /// Validate a configuration that is not being started: every value is read and checked
    /// like an enabled one, and a missing backend is only an error when `require_backend`.
    pub fn check(env: Lookup, require_backend: bool) -> Result<(), ConfigError> {
        let c = Self::read(env, true)?;
        if require_backend {
            c.validate_backend()?;
        }
        c.validate_limits()
    }

    fn validate_backend(&self) -> Result<(), ConfigError> {
        if self.jellyfin.is_none() && self.plex.is_none() {
            return Err(ConfigError::Inconsistent(
                "FlickDD needs at least one backend: set FLICKDD_JELLYFIN_URL and FLICKDD_JELLYFIN_API_KEY, or FLICKDD_PLEX_URL and FLICKDD_PLEX_TOKEN".to_owned(),
            ));
        }
        Ok(())
    }

    /// Coherence of an enabled configuration: a backend, and limits that make sense.
    fn validate(&self) -> Result<(), ConfigError> {
        self.validate_backend()?;
        self.validate_limits()
    }

    fn validate_limits(&self) -> Result<(), ConfigError> {
        const MIB: u64 = 1024 * 1024;
        let c = self;
        for (name, zero) in [
            ("FLICKDD_MAX_PARALLEL", c.max_parallel == 0),
            ("FLICKDD_MAX_GLOBAL", c.max_global == 0),
            ("FLICKDD_RATE_MBPS", c.rate_bps == 0),
            ("FLICKDD_CHUNK_MB", c.chunk_bytes == 0),
            ("FLICKDD_MAX_RANGE_MB", c.max_range_bytes == 0),
            ("FLICKDD_MAX_REQUESTS_PER_MIN", c.max_requests_per_min == 0),
            ("FLICKDD_MAX_OVERSERVE", c.max_overserve == 0),
            ("FLICKDD_STALL_TIMEOUT", c.stall_timeout_ms == 0),
            ("FLICKDD_UPSTREAM_TIMEOUT", c.upstream_timeout_ms == 0),
        ] {
            if zero {
                return Err(ConfigError::Inconsistent(format!(
                    "{name} must be at least 1; it is now 0"
                )));
            }
        }
        if c.chunk_bytes > c.max_range_bytes {
            return Err(ConfigError::Inconsistent(format!(
                "FLICKDD_CHUNK_MB ({}) must not exceed FLICKDD_MAX_RANGE_MB ({})",
                c.chunk_bytes / MIB,
                c.max_range_bytes / MIB
            )));
        }
        if c.max_parallel > c.max_global {
            return Err(ConfigError::Inconsistent(format!(
                "FLICKDD_MAX_PARALLEL ({}) must not exceed FLICKDD_MAX_GLOBAL ({})",
                c.max_parallel, c.max_global
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn check_validates_even_while_disabled() {
        let m = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        // Limits are checked although FLICKDD_ENABLED is not set.
        assert!(DdConfig::check(&m(&[("FLICKDD_MAX_PARALLEL", "0")]), false).is_err());
        // No backend is fine when the module is not enabled, refused when it is.
        assert!(DdConfig::check(&m(&[]), false).is_ok());
        assert!(DdConfig::check(&m(&[]), true).is_err());
        assert!(
            DdConfig::check(
                &m(&[
                    ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
                    ("FLICKDD_JELLYFIN_API_KEY", "k")
                ]),
                true
            )
            .is_ok()
        );
        // A half-configured backend is always an error.
        assert!(DdConfig::check(&m(&[("FLICKDD_PLEX_URL", "http://plex")]), false).is_err());
    }
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<DdConfig, ConfigError> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        DdConfig::from_lookup(&|k| m.get(k).cloned())
    }

    /// `vars` on top of an enabled configuration with a Jellyfin backend.
    fn enabled(vars: &[(&str, &str)]) -> Result<DdConfig, ConfigError> {
        let mut all = vec![
            ("FLICKDD_ENABLED", "true"),
            ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
            ("FLICKDD_JELLYFIN_API_KEY", "abc"),
        ];
        all.extend_from_slice(vars);
        cfg(&all)
    }

    #[test]
    fn defaults_match_the_spec() {
        let c = cfg(&[]).unwrap();
        assert!(!c.enabled);
        assert_eq!(c.max_parallel, 10);
        assert_eq!(c.max_global, 100);
        assert_eq!(c.rate_bps, 10 * 1024 * 1024);
        assert_eq!(c.chunk_bytes, 8 * 1024 * 1024);
        assert_eq!(c.max_range_bytes, 64 * 1024 * 1024);
        assert_eq!(c.grant_ttl_ms, 21_600_000);
        assert_eq!(c.grant_max_age_ms, 86_400_000);
        assert_eq!(c.stall_timeout_ms, 30_000);
        assert_eq!(c.upstream_retries, 2);
        assert_eq!(c.max_requests_per_min, 120);
        assert_eq!(c.max_overserve, 2);
    }

    #[test]
    fn enabled_requires_a_backend() {
        assert!(cfg(&[("FLICKDD_ENABLED", "true")]).is_err());
        let c = cfg(&[
            ("FLICKDD_ENABLED", "true"),
            ("FLICKDD_JELLYFIN_URL", "http://jf:8096/"),
            ("FLICKDD_JELLYFIN_API_KEY", "abc"),
        ])
        .unwrap();
        assert_eq!(c.jellyfin.unwrap().url, "http://jf:8096"); // trailing slash trimmed
    }

    #[test]
    fn a_backend_needs_both_url_and_secret() {
        assert!(enabled(&[("FLICKDD_PLEX_URL", "http://plex:32400")]).is_err());
        assert!(enabled(&[("FLICKDD_PLEX_TOKEN", "t")]).is_err());
    }

    #[test]
    fn rejects_nonsense_limits() {
        assert!(enabled(&[]).is_ok());
        assert!(enabled(&[("FLICKDD_MAX_PARALLEL", "0")]).is_err());
        assert!(enabled(&[("FLICKDD_RATE_MBPS", "0")]).is_err());
        assert!(enabled(&[("FLICKDD_CHUNK_MB", "0")]).is_err());
        assert!(enabled(&[("FLICKDD_CHUNK_MB", "128"), ("FLICKDD_MAX_RANGE_MB", "64")]).is_err());
        assert!(enabled(&[("FLICKDD_MAX_OVERSERVE", "0")]).is_err());
        assert!(enabled(&[("FLICKDD_MAX_PARALLEL", "20"), ("FLICKDD_MAX_GLOBAL", "10")]).is_err());
        assert!(enabled(&[("FLICKDD_MAX_PARALLEL", "ten")]).is_err());
    }

    #[test]
    fn timeouts_must_be_at_least_one_second() {
        assert!(enabled(&[("FLICKDD_STALL_TIMEOUT", "0")]).is_err());
        assert!(enabled(&[("FLICKDD_UPSTREAM_TIMEOUT", "0")]).is_err());
        assert!(
            enabled(&[
                ("FLICKDD_STALL_TIMEOUT", "1"),
                ("FLICKDD_UPSTREAM_TIMEOUT", "1")
            ])
            .is_ok()
        );
    }

    #[test]
    fn disabled_ignores_every_other_flickdd_variable() {
        // Each of these refuses an enabled configuration; none may block a disabled one.
        let broken: &[&[(&str, &str)]] = &[
            &[("FLICKDD_PLEX_URL", "http://plex:32400")],
            &[("FLICKDD_PLEX_TOKEN", "t")],
            &[("FLICKDD_MAX_PARALLEL", "0")],
            &[("FLICKDD_MAX_PARALLEL", "ten")],
            &[("FLICKDD_RATE_MBPS", "-1")],
            &[("FLICKDD_CHUNK_MB", "128"), ("FLICKDD_MAX_RANGE_MB", "64")],
            &[("FLICKDD_MAX_PARALLEL", "20"), ("FLICKDD_MAX_GLOBAL", "10")],
            &[("FLICKDD_STALL_TIMEOUT", "0")],
        ];
        for vars in broken {
            assert!(
                enabled(vars).is_err(),
                "{vars:?} must be refused when enabled"
            );
            for flag in [None, Some("false"), Some("0")] {
                let mut all = vars.to_vec();
                all.extend(flag.map(|f| ("FLICKDD_ENABLED", f)));
                let c = cfg(&all).unwrap_or_else(|e| panic!("{all:?}: {e}"));
                assert!(!c.enabled);
                assert!(c.jellyfin.is_none() && c.plex.is_none(), "{all:?}");
                assert_eq!(c.max_parallel, 10, "{all:?}: defaults apply");
            }
        }
    }

    #[test]
    fn debug_never_prints_backend_secrets() {
        let c = cfg(&[
            ("FLICKDD_ENABLED", "true"),
            ("FLICKDD_JELLYFIN_URL", "http://jf:8096"),
            ("FLICKDD_JELLYFIN_API_KEY", "s3cr3t-jf-key"),
            ("FLICKDD_PLEX_URL", "http://plex:32400"),
            ("FLICKDD_PLEX_TOKEN", "s3cr3t-plex-token"),
        ])
        .unwrap();
        let out = format!("{c:?}");
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(out.contains("http://jf:8096"), "{out}");
    }
}

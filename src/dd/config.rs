//! FlickDD configuration (`FLICKDD_*` environment variables).

use crate::config::{ConfigError, Lookup, parse, parse_bool};

#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub url: String,
    pub secret: String,
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
            "{label} backend needs both {url_var} and {secret_var}, or neither"
        ))),
    }
}

impl DdConfig {
    pub fn from_lookup(env: Lookup) -> Result<Self, ConfigError> {
        const MIB: u64 = 1024 * 1024;
        let inconsistent = |m: &str| ConfigError::Inconsistent(m.to_owned());

        let enabled = parse_bool(env, "FLICKDD_ENABLED", false)?;
        let jellyfin = backend(
            env,
            "Jellyfin",
            "FLICKDD_JELLYFIN_URL",
            "FLICKDD_JELLYFIN_API_KEY",
        )?;
        let plex = backend(env, "Plex", "FLICKDD_PLEX_URL", "FLICKDD_PLEX_TOKEN")?;
        if enabled && jellyfin.is_none() && plex.is_none() {
            return Err(inconsistent(
                "FLICKDD_ENABLED=true requires at least one backend (Jellyfin or Plex)",
            ));
        }

        let c = Self {
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
        };

        if c.max_parallel == 0
            || c.max_global == 0
            || c.rate_bps == 0
            || c.chunk_bytes == 0
            || c.max_range_bytes == 0
            || c.max_requests_per_min == 0
            || c.max_overserve == 0
        {
            return Err(inconsistent(
                "FLICKDD limits (parallel, global, rate, chunk, range, requests/min, overserve) must be >= 1",
            ));
        }
        if c.chunk_bytes > c.max_range_bytes {
            return Err(inconsistent(
                "FLICKDD_CHUNK_MB must not exceed FLICKDD_MAX_RANGE_MB",
            ));
        }
        if c.max_parallel > c.max_global {
            return Err(inconsistent(
                "FLICKDD_MAX_PARALLEL must not exceed FLICKDD_MAX_GLOBAL",
            ));
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<DdConfig, ConfigError> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        DdConfig::from_lookup(&|k| m.get(k).cloned())
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
        assert!(cfg(&[("FLICKDD_PLEX_URL", "http://plex:32400")]).is_err());
        assert!(cfg(&[("FLICKDD_PLEX_TOKEN", "t")]).is_err());
    }

    #[test]
    fn rejects_nonsense_limits() {
        assert!(cfg(&[("FLICKDD_MAX_PARALLEL", "0")]).is_err());
        assert!(cfg(&[("FLICKDD_RATE_MBPS", "0")]).is_err());
        assert!(cfg(&[("FLICKDD_CHUNK_MB", "0")]).is_err());
        assert!(cfg(&[("FLICKDD_CHUNK_MB", "128"), ("FLICKDD_MAX_RANGE_MB", "64")]).is_err());
        assert!(cfg(&[("FLICKDD_MAX_OVERSERVE", "0")]).is_err());
    }
}

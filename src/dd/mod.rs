//! FlickDD: throttled, resumable direct downloads proxied from Jellyfin / Plex.

pub mod backend;
pub mod config;
pub mod grants;
pub mod pump;
pub mod ranges;
pub mod stats;
pub mod throttle;
pub mod types;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use backend::Backends;
use config::DdConfig;
use grants::{GrantError, Grants};
use stats::Stats;
use throttle::Micros;

use crate::errors::{Error, ErrorCode};
use crate::ratelimit::TokenBucket;

/// Grant creations (`POST /api/v1/downloads`) one user may attempt per minute, also the
/// burst size. A real client creates a grant per download, at most `FLICKDD_MAX_PARALLEL`
/// at a time, and backs off on refusals: 30 a minute leaves it ample room while a client
/// stuck in a create loop is stopped before it hammers Jellyfin / Plex.
pub const CREATE_PER_MINUTE: u32 = 30;

/// Everything FlickDD shares between requests.
pub struct DdState {
    pub cfg: Arc<DdConfig>,
    pub grants: Grants,
    pub stats: Arc<Stats>,
    pub backends: Backends,
    /// Per-user token buckets of [`CREATE_PER_MINUTE`], keyed like the grant owner.
    create_limits: Mutex<HashMap<String, TokenBucket>>,
    epoch: Instant,
}

impl DdState {
    pub fn new(cfg: DdConfig) -> Arc<DdState> {
        let cfg = Arc::new(cfg);
        let stats = Arc::new(Stats::new());
        Arc::new(DdState {
            grants: Grants::new(cfg.clone(), stats.clone()),
            backends: Backends::new(&cfg),
            stats,
            cfg,
            create_limits: Mutex::new(HashMap::new()),
            epoch: Instant::now(),
        })
    }

    /// Drop expired grants and forget idle creation buckets (called from the app sweeper).
    pub fn sweep(&self) {
        self.grants.sweep(now_ms());
        let now = self.now_mono_ms();
        self.create_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, b| !b.is_idle(now));
    }

    /// Server shutdown: cut every running download stream and refuse new grants and new
    /// streams (see [`Grants::shutdown`]). Returns the number of streams cut.
    pub fn shutdown(&self) -> usize {
        self.grants.shutdown()
    }

    /// Take one creation token of `user_key` at `now` (monotonic ms, see
    /// [`DdState::now_mono_ms`]). Refused with `RateLimited` and a `Retry-After` hint.
    pub fn admit_create(&self, user_key: &str, now: u64) -> Result<(), GrantError> {
        let mut limits = self.create_limits.lock().unwrap_or_else(|e| e.into_inner());
        let per_min = f64::from(CREATE_PER_MINUTE);
        let bucket = limits
            .entry(user_key.to_owned())
            .or_insert_with(|| TokenBucket::new(per_min, per_min / 60.0, now));
        if bucket.try_acquire(now) {
            return Ok(());
        }
        let wait_ms = bucket.wait_ms(now);
        drop(limits);
        self.stats.record_rejected("create");
        Err(GrantError {
            error: Error::new(
                ErrorCode::RateLimited,
                "too many downloads created, try again later",
            ),
            retry_after_secs: Some(wait_ms.div_ceil(1000).max(1)),
        })
    }

    /// Monotonic milliseconds since this state was created (creation rate limit clock).
    pub fn now_mono_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Monotonic microseconds since this state was created (throttle clock).
    pub fn now_us(&self) -> Micros {
        self.epoch.elapsed().as_micros() as Micros
    }
}

/// Wall clock, milliseconds since the UNIX epoch (grant and analytics clock).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<DdState> {
        DdState::new(DdConfig::from_lookup(&|_| None).unwrap())
    }

    #[test]
    fn creation_is_rate_limited_per_user_with_a_retry_hint() {
        let dd = state();
        for _ in 0..CREATE_PER_MINUTE {
            dd.admit_create("srv/alice", 0).unwrap();
        }
        let e = dd.admit_create("srv/alice", 0).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::RateLimited);
        // One token every 2 s at 30 a minute.
        assert_eq!(e.retry_after_secs, Some(2));
        assert_eq!(dd.stats.snapshot(0).rejected.get("create"), Some(&1));
        // Another user (or the same id on another Flick server) has its own budget.
        dd.admit_create("srv/bob", 0).unwrap();
        dd.admit_create("other/alice", 0).unwrap();
        // The bucket refills.
        dd.admit_create("srv/alice", 2_000).unwrap();
        assert!(dd.admit_create("srv/alice", 2_000).is_err());
    }
}

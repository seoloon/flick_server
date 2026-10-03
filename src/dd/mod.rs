//! FlickDD: throttled, resumable direct downloads proxied from Jellyfin / Plex.

pub mod backend;
pub mod config;
pub mod grants;
pub mod pump;
pub mod ranges;
pub mod stats;
pub mod throttle;
pub mod types;

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use backend::Backends;
use config::DdConfig;
use grants::Grants;
use stats::Stats;
use throttle::Micros;

/// Everything FlickDD shares between requests.
pub struct DdState {
    pub cfg: Arc<DdConfig>,
    pub grants: Grants,
    pub stats: Arc<Stats>,
    pub backends: Backends,
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
            epoch: Instant::now(),
        })
    }

    /// Drop expired grants (called from the app sweeper).
    pub fn sweep(&self) {
        self.grants.sweep(now_ms());
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

//! Time sources.
//!
//! The server measures every *duration* with a monotonic clock. Wall-clock
//! timestamps are derived from a single base captured at start-up plus the
//! monotonic elapsed time, so they never jump when the OS clock is adjusted;
//! they exist only to be communicated to clients and for debugging.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Milliseconds.
pub type Millis = u64;

/// A reading of the clock: monotonic ms since start-up and wall-clock ms since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Time {
    pub mono_ms: Millis,
    pub wall_ms: Millis,
}

/// Injectable time source (real in production, manual in tests).
pub trait Clock: Send + Sync {
    fn now(&self) -> Time;
}

pub struct SystemClock {
    start: Instant,
    wall_base_ms: Millis,
}

impl SystemClock {
    pub fn new() -> Self {
        let wall_base_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            start: Instant::now(),
            wall_base_ms,
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Time {
        let mono_ms = self.start.elapsed().as_millis() as u64;
        Time {
            mono_ms,
            wall_ms: self.wall_base_ms + mono_ms,
        }
    }
}

/// Deterministic clock for tests.
pub struct ManualClock {
    mono_ms: AtomicU64,
    wall_base_ms: Millis,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            mono_ms: AtomicU64::new(0),
            wall_base_ms: 1_700_000_000_000,
        }
    }

    pub fn advance(&self, ms: Millis) {
        self.mono_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Time {
        let mono_ms = self.mono_ms.load(Ordering::SeqCst);
        Time {
            mono_ms,
            wall_ms: self.wall_base_ms + mono_ms,
        }
    }
}

/// Reference client-side logic, kept here so the documented algorithm is tested.
///
/// A client estimates the offset between its own clock and the server wall
/// clock from ping/pong round trips, then derives the canonical position from
/// a `PlaybackSnapshot` without any further traffic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServerClockEstimate {
    /// `server_wall - client_wall`, in ms.
    pub offset_ms: f64,
    pub rtt_ms: f64,
}

impl ServerClockEstimate {
    /// `client_sent`/`client_recv` are client clock readings around a ping;
    /// `server_time` is the `server_time` carried by the matching pong.
    /// The server stamped the pong roughly half an RTT after the ping left.
    pub fn from_ping(client_sent_ms: f64, client_recv_ms: f64, server_time_ms: f64) -> Self {
        let rtt_ms = (client_recv_ms - client_sent_ms).max(0.0);
        let offset_ms = server_time_ms - (client_sent_ms + rtt_ms / 2.0);
        Self { offset_ms, rtt_ms }
    }

    /// Server wall-clock time corresponding to a client clock reading.
    pub fn server_now(&self, client_now_ms: f64) -> f64 {
        client_now_ms + self.offset_ms
    }
}

/// Canonical position (seconds) at `server_now_ms`, derived from a snapshot.
pub fn position_from_snapshot(
    position: f64,
    playing: bool,
    rate: f64,
    snapshot_server_time_ms: f64,
    server_now_ms: f64,
) -> f64 {
    if !playing {
        return position;
    }
    let elapsed_s = (server_now_ms - snapshot_server_time_ms).max(0.0) / 1000.0;
    position + elapsed_s * rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_is_deterministic() {
        let c = ManualClock::new();
        let a = c.now();
        c.advance(500);
        let b = c.now();
        assert_eq!(b.mono_ms - a.mono_ms, 500);
        assert_eq!(b.wall_ms - a.wall_ms, 500);
    }

    #[test]
    fn system_clock_is_monotonic() {
        let c = SystemClock::new();
        let a = c.now();
        let b = c.now();
        assert!(b.mono_ms >= a.mono_ms);
    }

    // Snapshot says: 100.0s at server_time T, playing. The client receives it
    // 500ms after it was stamped.
    fn target_for_rtt(rtt: f64, rate: f64) -> f64 {
        let t = 1_000_000.0;
        // Client clock is 7s ahead of the server, to prove the offset is cancelled out.
        let client_skew = 7_000.0;
        // Ping sent at server time t, pong stamped by server at t + rtt/2.
        let est =
            ServerClockEstimate::from_ping(t + client_skew, t + client_skew + rtt, t + rtt / 2.0);
        assert!((est.offset_ms + client_skew).abs() < 1e-6);
        assert!((est.rtt_ms - rtt).abs() < 1e-6);
        let snapshot_time = t + 1_000.0;
        let client_now = snapshot_time + 500.0 + client_skew;
        position_from_snapshot(100.0, true, rate, snapshot_time, est.server_now(client_now))
    }

    #[test]
    fn target_position_accounts_for_elapsed_time_at_various_rtts() {
        for rtt in [10.0, 50.0, 100.0, 300.0] {
            let got = target_for_rtt(rtt, 1.0);
            assert!((got - 100.5).abs() < 1e-6, "rtt={rtt} got={got}");
        }
    }

    #[test]
    fn target_position_at_rate_1_5() {
        let got = target_for_rtt(50.0, 1.5);
        assert!((got - 100.75).abs() < 1e-6, "got={got}");
    }

    #[test]
    fn asymmetric_delay_error_is_bounded_by_half_rtt() {
        // Pong delayed entirely on the way back: all RTT on the return leg.
        let est = ServerClockEstimate::from_ping(0.0, 300.0, 0.0);
        // True offset 0, estimate is off by rtt/2 = 150ms at worst.
        assert!(est.offset_ms.abs() <= 150.0 + 1e-6);
    }

    #[test]
    fn paused_position_is_constant() {
        assert_eq!(
            position_from_snapshot(42.0, false, 1.0, 0.0, 99_999.0),
            42.0
        );
    }

    #[test]
    fn reconnect_after_five_seconds_catches_up() {
        let got = position_from_snapshot(10.0, true, 1.0, 0.0, 5_000.0);
        assert!((got - 15.0).abs() < 1e-9);
    }
}

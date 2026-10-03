//! Token bucket with debt, driven by an explicit microsecond timestamp (pure, testable).

pub type Micros = u64;

#[derive(Debug, Clone)]
pub struct Throttle {
    per_us: f64,
    burst: f64,
    tokens: f64,
    last: Micros,
}

impl Throttle {
    pub fn new(rate_bps: u64, burst_bytes: u64, now: Micros) -> Self {
        Self {
            per_us: rate_bps as f64 / 1e6,
            burst: burst_bytes as f64,
            tokens: burst_bytes as f64,
            last: now,
        }
    }

    /// Reserve `bytes`; returns how long the caller must wait before sending them.
    pub fn take(&mut self, bytes: u64, now: Micros) -> Micros {
        let elapsed = now.saturating_sub(self.last) as f64;
        self.last = self.last.max(now);
        self.tokens = (self.tokens + elapsed * self.per_us).min(self.burst);
        self.tokens -= bytes as f64;
        if self.tokens >= 0.0 {
            0
        } else {
            (-self.tokens / self.per_us).ceil() as Micros
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn burst_is_free_then_throttled() {
        let mut t = Throttle::new(10 * MIB, 256 * 1024, 0);
        assert_eq!(t.take(256 * 1024, 0), 0);
        // Next 64 KiB must wait 64K / 10M s = 6250 us.
        assert_eq!(t.take(64 * 1024, 0), 6_250);
    }

    #[test]
    fn sustained_rate_equals_the_limit() {
        let mut t = Throttle::new(10 * MIB, 64 * 1024, 0);
        let mut now = 0u64;
        let mut sent = 0u64;
        while sent < 100 * MIB {
            let wait = t.take(64 * 1024, now);
            now += wait;
            sent += 64 * 1024;
        }
        // 100 MiB at 10 MiB/s = 10 s (minus the free burst), within 2 %.
        let secs = now as f64 / 1e6;
        assert!((secs - 10.0).abs() < 0.2, "took {secs}s");
    }

    #[test]
    fn idle_time_refills_up_to_the_burst_only() {
        let mut t = Throttle::new(10 * MIB, 256 * 1024, 0);
        t.take(256 * 1024, 0);
        assert_eq!(t.take(256 * 1024, 10_000_000), 0); // 10 s later: full again
        assert!(t.take(256 * 1024, 10_000_000) > 0); // but never more than the burst
    }

    #[test]
    fn time_going_backwards_is_harmless() {
        let mut t = Throttle::new(MIB, 1024, 1_000);
        assert_eq!(t.take(1024, 500), 0);
    }
}

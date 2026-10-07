//! Token-bucket rate limiter driven by an explicit monotonic timestamp.

use crate::sync::clock::Millis;

#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_ms: f64,
    last: Millis,
}

impl TokenBucket {
    /// `capacity` is the burst size, `per_second` the sustained rate.
    pub fn new(capacity: f64, per_second: f64, now: Millis) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_per_ms: per_second / 1000.0,
            last: now,
        }
    }

    pub fn try_acquire(&mut self, now: Millis) -> bool {
        let elapsed = now.saturating_sub(self.last) as f64;
        self.last = self.last.max(now);
        self.tokens = (self.tokens + elapsed * self.refill_per_ms).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Milliseconds until the next token is available (0 when one is available now).
    pub fn wait_ms(&self, now: Millis) -> Millis {
        let elapsed = now.saturating_sub(self.last) as f64;
        let tokens = (self.tokens + elapsed * self.refill_per_ms).min(self.capacity);
        if tokens >= 1.0 || self.refill_per_ms <= 0.0 {
            return 0;
        }
        ((1.0 - tokens) / self.refill_per_ms).ceil() as Millis
    }

    /// True when the bucket is full again (safe to forget about it).
    pub fn is_idle(&self, now: Millis) -> bool {
        let elapsed = now.saturating_sub(self.last) as f64;
        self.tokens + elapsed * self.refill_per_ms >= self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_burst_then_throttles_then_refills() {
        let mut b = TokenBucket::new(3.0, 1.0, 0);
        assert!(b.try_acquire(0));
        assert!(b.try_acquire(0));
        assert!(b.try_acquire(0));
        assert!(!b.try_acquire(0));
        assert!(!b.try_acquire(500));
        assert!(b.try_acquire(1_000));
        assert!(!b.try_acquire(1_000));
    }

    #[test]
    fn never_exceeds_capacity() {
        let mut b = TokenBucket::new(2.0, 100.0, 0);
        assert!(b.try_acquire(1_000_000));
        assert!(b.try_acquire(1_000_000));
        assert!(!b.try_acquire(1_000_000));
    }

    #[test]
    fn wait_until_the_next_token() {
        let mut b = TokenBucket::new(2.0, 0.5, 0); // one token every 2 s
        assert_eq!(b.wait_ms(0), 0);
        assert!(b.try_acquire(0) && b.try_acquire(0));
        assert_eq!(b.wait_ms(0), 2_000);
        assert_eq!(b.wait_ms(1_500), 500);
        assert_eq!(b.wait_ms(2_000), 0);
    }

    #[test]
    fn idle_detection() {
        let mut b = TokenBucket::new(2.0, 1.0, 0);
        b.try_acquire(0);
        assert!(!b.is_idle(0));
        assert!(b.is_idle(1_000));
    }
}

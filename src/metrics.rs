//! Minimal in-process metrics (plain atomics) with an optional Prometheus text rendering.
//! No metrics crate: the surface is small and the exposition format is trivial.

use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

#[derive(Default)]
pub struct Counter(AtomicU64);

impl Counter {
    pub fn inc(&self) {
        self.add(1);
    }
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Gauge(AtomicU64);

impl Gauge {
    pub fn set(&self, v: u64) {
        self.0.store(v, Ordering::Relaxed);
    }
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    pub fn dec(&self) {
        // Saturating: a bookkeeping bug must never wrap to u64::MAX.
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(1))
            });
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Metrics {
    pub rooms_active: Gauge,
    pub participants_active: Gauge,
    pub ws_connections: Gauge,
    /// Average smoothed client RTT in microseconds.
    pub rtt_avg_us: Gauge,
    pub rooms_created_total: Counter,
    pub rooms_destroyed_total: Counter,
    pub messages_in_total: Counter,
    pub sync_corrections_total: Counter,
    pub auth_failures_total: Counter,
    pub rate_limited_total: Counter,
    pub malformed_messages_total: Counter,
    /// Drift corrections that were hard seeks (the rest are rate adjustments).
    pub sync_seeks_total: Counter,
    /// Sync reports that were evaluated against the room clock.
    pub sync_reports_total: Counter,
    /// Evaluated reports by absolute drift: below ignore, below soft, below hard, hard and above.
    pub drift_buckets: [Counter; 4],
    pub history: History,
}

/// One point of the rolling history (cumulative counters; consumers compute rates).
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    /// Wall clock, ms since the Unix epoch.
    pub t: u64,
    pub rooms: u64,
    pub participants: u64,
    pub connections: u64,
    pub rtt_ms: f64,
    pub messages_in: u64,
    pub corrections: u64,
    pub seeks: u64,
    pub reports: u64,
}

/// Fixed-size ring of samples: bounded memory, no external store.
pub struct History {
    samples: Mutex<VecDeque<Sample>>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(Self::CAPACITY)),
        }
    }
}

impl History {
    /// One hour at one sample every 10 s.
    pub const CAPACITY: usize = 360;
    pub const INTERVAL_MS: u64 = 10_000;

    /// Append `s` unless the previous sample is younger than the interval. Returns true when stored.
    pub fn record(&self, s: Sample) -> bool {
        let mut q = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        if q.back()
            .is_some_and(|l| s.t.saturating_sub(l.t) < Self::INTERVAL_MS)
        {
            return false;
        }
        if q.len() == Self::CAPACITY {
            q.pop_front();
        }
        q.push_back(s);
        true
    }

    pub fn snapshot(&self) -> Vec<Sample> {
        self.samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

impl Metrics {
    pub fn render_prometheus(&self) -> String {
        let mut s = String::new();
        let mut put = |name: &str, kind: &str, help: &str, v: f64| {
            let _ = writeln!(s, "# HELP flicksync_{name} {help}");
            let _ = writeln!(s, "# TYPE flicksync_{name} {kind}");
            let _ = writeln!(s, "flicksync_{name} {v}");
        };
        put(
            "rooms_active",
            "gauge",
            "Rooms currently alive",
            self.rooms_active.get() as f64,
        );
        put(
            "participants_active",
            "gauge",
            "Participants across all rooms",
            self.participants_active.get() as f64,
        );
        put(
            "websocket_connections",
            "gauge",
            "Open WebSocket connections",
            self.ws_connections.get() as f64,
        );
        put(
            "rtt_average_milliseconds",
            "gauge",
            "Average smoothed client RTT",
            self.rtt_avg_us.get() as f64 / 1000.0,
        );
        put(
            "rooms_created_total",
            "counter",
            "Rooms created",
            self.rooms_created_total.get() as f64,
        );
        put(
            "rooms_destroyed_total",
            "counter",
            "Rooms destroyed",
            self.rooms_destroyed_total.get() as f64,
        );
        put(
            "messages_received_total",
            "counter",
            "Client messages received",
            self.messages_in_total.get() as f64,
        );
        put(
            "sync_corrections_total",
            "counter",
            "Drift corrections issued",
            self.sync_corrections_total.get() as f64,
        );
        put(
            "sync_seeks_total",
            "counter",
            "Drift corrections that were hard seeks",
            self.sync_seeks_total.get() as f64,
        );
        put(
            "sync_reports_total",
            "counter",
            "Sync reports evaluated against the room clock",
            self.sync_reports_total.get() as f64,
        );
        put(
            "auth_failures_total",
            "counter",
            "Failed authentications",
            self.auth_failures_total.get() as f64,
        );
        put(
            "rate_limited_total",
            "counter",
            "Requests/messages rejected by rate limits",
            self.rate_limited_total.get() as f64,
        );
        put(
            "malformed_messages_total",
            "counter",
            "Malformed client messages",
            self.malformed_messages_total.get() as f64,
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_never_underflows() {
        let g = Gauge::default();
        g.dec();
        assert_eq!(g.get(), 0);
        g.inc();
        g.inc();
        g.dec();
        assert_eq!(g.get(), 1);
    }

    #[test]
    fn history_is_bounded_and_rate_limited() {
        let h = History::default();
        let s = |t| Sample {
            t,
            rooms: 0,
            participants: 0,
            connections: 0,
            rtt_ms: 0.0,
            messages_in: 0,
            corrections: 0,
            seeks: 0,
            reports: 0,
        };
        assert!(h.record(s(0)));
        assert!(!h.record(s(History::INTERVAL_MS - 1)), "too soon");
        for i in 1..(History::CAPACITY as u64 + 20) {
            assert!(h.record(s(i * History::INTERVAL_MS)));
        }
        let snap = h.snapshot();
        assert_eq!(snap.len(), History::CAPACITY);
        assert!(snap.windows(2).all(|w| w[0].t < w[1].t));
    }

    #[test]
    fn renders_prometheus_text() {
        let m = Metrics::default();
        m.rooms_created_total.inc();
        let t = m.render_prometheus();
        assert!(t.contains("flicksync_rooms_created_total 1"));
        assert!(t.contains("# TYPE flicksync_rooms_active gauge"));
    }
}

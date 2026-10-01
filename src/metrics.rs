//! Minimal in-process metrics (plain atomics) with an optional Prometheus text rendering.
//! No metrics crate: the surface is small and the exposition format is trivial.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

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
    fn renders_prometheus_text() {
        let m = Metrics::default();
        m.rooms_created_total.inc();
        let t = m.render_prometheus();
        assert!(t.contains("flicksync_rooms_created_total 1"));
        assert!(t.contains("# TYPE flicksync_rooms_active gauge"));
    }
}

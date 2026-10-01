//! Drift detection and correction policy.
//!
//! Clients send occasional `sync_report`s with their local position. The
//! server compares them against the canonical position and answers with the
//! gentlest correction that works:
//!
//! | |drift|        | action                                   |
//! |----------------|------------------------------------------|
//! | < `ignore_ms`  | nothing                                  |
//! | < `soft_ms`    | temporary small rate adjustment          |
//! | < `hard_ms`    | temporary stronger rate adjustment       |
//! | >= `hard_ms`   | seek to the canonical position           |
//!
//! Hard seeks are rate-limited by a cooldown so a struggling client is not
//! seeked in a loop, and a rate adjustment is not re-issued while the previous
//! one is still running.

use super::clock::{Millis, Time};
use super::playback::PlaybackStatus;

#[derive(Debug, Clone)]
pub struct DriftConfig {
    pub ignore_ms: f64,
    pub soft_ms: f64,
    pub hard_ms: f64,
    /// Relative rate change for the soft band (0.03 = ±3%).
    pub rate_soft: f64,
    /// Relative rate change for the strong band.
    pub rate_strong: f64,
    pub seek_cooldown_ms: Millis,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            ignore_ms: 100.0,
            soft_ms: 500.0,
            hard_ms: 1500.0,
            rate_soft: 0.03,
            rate_strong: 0.08,
            seek_cooldown_ms: 3_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Correction {
    /// Play at `rate` for `duration_ms`, then return to the canonical rate.
    AdjustRate { rate: f64, duration_ms: Millis },
    /// Jump to `position` (already compensated for network latency).
    Seek { position: f64 },
}

/// What the server knows about the canonical state when evaluating a report.
#[derive(Debug, Clone, Copy)]
pub struct Canonical {
    pub status: PlaybackStatus,
    pub position: f64,
    pub rate: f64,
}

/// A participant's own observation.
#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub position: f64,
    pub rtt_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// Signed drift in ms: positive when the client is ahead of the room.
    pub drift_ms: f64,
    pub correction: Option<Correction>,
}

/// Pure policy: no state, no clock.
pub fn evaluate(cfg: &DriftConfig, canonical: Canonical, obs: Observation) -> Evaluation {
    // The report spent about rtt/2 in flight; a playing client has advanced by then.
    let in_flight_s = obs.rtt_ms / 2.0 / 1000.0;
    let playing = canonical.status == PlaybackStatus::Playing;
    let client_now = if playing {
        obs.position + in_flight_s * canonical.rate
    } else {
        obs.position
    };
    let drift_ms = (client_now - canonical.position) * 1000.0;
    let abs = drift_ms.abs();

    let correction = if !playing {
        // Nothing moves while paused: no rate games, just realign if visibly off.
        (abs >= cfg.soft_ms).then_some(Correction::Seek {
            position: canonical.position,
        })
    } else if abs < cfg.ignore_ms {
        None
    } else if abs >= cfg.hard_ms {
        // By the time the correction arrives the room has moved on by rtt/2 again.
        Some(Correction::Seek {
            position: canonical.position + in_flight_s * canonical.rate,
        })
    } else {
        let factor = if abs < cfg.soft_ms {
            cfg.rate_soft
        } else {
            cfg.rate_strong
        };
        // Ahead -> slow down, behind -> speed up.
        let adjusted = if drift_ms > 0.0 {
            canonical.rate * (1.0 - factor)
        } else {
            canonical.rate * (1.0 + factor)
        };
        // Closing speed is factor * canonical rate (seconds of media per second).
        let duration_ms = (abs / (factor * canonical.rate))
            .clamp(500.0, 30_000.0)
            .round() as Millis;
        Some(Correction::AdjustRate {
            rate: adjusted,
            duration_ms,
        })
    };

    Evaluation {
        drift_ms,
        correction,
    }
}

/// Per-participant state: smoothed RTT and correction hysteresis.
#[derive(Debug, Clone, Default)]
pub struct DriftTracker {
    rtt_ms: Option<f64>,
    rate_correction_until: Millis,
    last_seek: Option<Millis>,
}

impl DriftTracker {
    pub fn rtt_ms(&self) -> Option<f64> {
        self.rtt_ms
    }

    /// Feed a client-measured RTT sample (smoothed, clamped to a sane range).
    pub fn record_rtt(&mut self, sample_ms: f64) {
        if !sample_ms.is_finite() {
            return;
        }
        let s = sample_ms.clamp(0.0, 10_000.0);
        self.rtt_ms = Some(match self.rtt_ms {
            Some(prev) => prev * 0.7 + s * 0.3,
            None => s,
        });
    }

    /// Evaluate a report, applying cooldown/hysteresis. Returns the drift and
    /// the correction to send, if any.
    pub fn on_report(
        &mut self,
        cfg: &DriftConfig,
        now: Time,
        canonical: Canonical,
        position: f64,
    ) -> Evaluation {
        let obs = Observation {
            position,
            rtt_ms: self.rtt_ms.unwrap_or(0.0),
        };
        let mut eval = evaluate(cfg, canonical, obs);
        match eval.correction {
            Some(Correction::Seek { .. }) => {
                let cooling = self
                    .last_seek
                    .is_some_and(|t| now.mono_ms.saturating_sub(t) < cfg.seek_cooldown_ms);
                if cooling {
                    eval.correction = None;
                } else {
                    self.last_seek = Some(now.mono_ms);
                    self.rate_correction_until = 0;
                }
            }
            Some(Correction::AdjustRate { duration_ms, .. }) => {
                if now.mono_ms < self.rate_correction_until {
                    eval.correction = None;
                } else {
                    self.rate_correction_until = now.mono_ms + duration_ms;
                }
            }
            None => {}
        }
        eval
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(pos: f64) -> Canonical {
        Canonical {
            status: PlaybackStatus::Playing,
            position: pos,
            rate: 1.0,
        }
    }

    fn obs(pos: f64, rtt: f64) -> Observation {
        Observation {
            position: pos,
            rtt_ms: rtt,
        }
    }

    fn t(ms: u64) -> Time {
        Time {
            mono_ms: ms,
            wall_ms: ms,
        }
    }

    #[test]
    fn small_drift_is_ignored() {
        let cfg = DriftConfig::default();
        let e = evaluate(&cfg, canon(100.0), obs(100.05, 0.0));
        assert_eq!(e.correction, None);
        assert!((e.drift_ms - 50.0).abs() < 1e-6);
    }

    #[test]
    fn rtt_compensation_removes_apparent_drift() {
        let cfg = DriftConfig::default();
        // Client was 150ms behind when it measured, but the report took 300ms
        // to arrive (rtt 300 -> 150ms in flight): it is actually in sync.
        let e = evaluate(&cfg, canon(100.0), obs(99.85, 300.0));
        assert!(e.drift_ms.abs() < 1e-6);
        assert_eq!(e.correction, None);
    }

    #[test]
    fn client_ahead_slows_down_and_behind_speeds_up() {
        let cfg = DriftConfig::default();
        let ahead = evaluate(&cfg, canon(100.0), obs(100.3, 0.0));
        match ahead.correction {
            Some(Correction::AdjustRate { rate, duration_ms }) => {
                assert!((rate - 0.97).abs() < 1e-9);
                assert_eq!(duration_ms, 10_000);
            }
            other => panic!("unexpected {other:?}"),
        }
        let behind = evaluate(&cfg, canon(100.0), obs(99.7, 0.0));
        match behind.correction {
            Some(Correction::AdjustRate { rate, .. }) => assert!((rate - 1.03).abs() < 1e-9),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn medium_drift_uses_strong_rate() {
        let cfg = DriftConfig::default();
        let e = evaluate(&cfg, canon(100.0), obs(99.0, 0.0));
        match e.correction {
            Some(Correction::AdjustRate { rate, .. }) => assert!((rate - 1.08).abs() < 1e-9),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rate_correction_scales_with_canonical_rate() {
        let cfg = DriftConfig::default();
        let c = Canonical {
            status: PlaybackStatus::Playing,
            position: 100.0,
            rate: 1.5,
        };
        let e = evaluate(&cfg, c, obs(99.7, 0.0));
        match e.correction {
            Some(Correction::AdjustRate { rate, .. }) => assert!((rate - 1.545).abs() < 1e-9),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn large_drift_seeks_to_latency_compensated_target() {
        let cfg = DriftConfig::default();
        let e = evaluate(&cfg, canon(100.0), obs(90.0, 200.0));
        assert_eq!(e.correction, Some(Correction::Seek { position: 100.1 }));
    }

    #[test]
    fn paused_room_realigns_with_seek_only() {
        let cfg = DriftConfig::default();
        let c = Canonical {
            status: PlaybackStatus::Paused,
            position: 50.0,
            rate: 1.0,
        };
        assert_eq!(evaluate(&cfg, c, obs(50.2, 0.0)).correction, None);
        assert_eq!(
            evaluate(&cfg, c, obs(51.0, 0.0)).correction,
            Some(Correction::Seek { position: 50.0 })
        );
    }

    #[test]
    fn seek_cooldown_prevents_seek_loops() {
        let cfg = DriftConfig::default();
        let mut tr = DriftTracker::default();
        let e1 = tr.on_report(&cfg, t(10_000), canon(100.0), 80.0);
        assert!(matches!(e1.correction, Some(Correction::Seek { .. })));
        let e2 = tr.on_report(&cfg, t(11_000), canon(101.0), 80.5);
        assert_eq!(e2.correction, None, "within cooldown");
        let e3 = tr.on_report(&cfg, t(14_000), canon(104.0), 82.0);
        assert!(matches!(e3.correction, Some(Correction::Seek { .. })));
    }

    #[test]
    fn rate_correction_is_not_reissued_while_running() {
        let cfg = DriftConfig::default();
        let mut tr = DriftTracker::default();
        let e1 = tr.on_report(&cfg, t(0), canon(100.0), 99.7);
        assert!(matches!(e1.correction, Some(Correction::AdjustRate { .. })));
        let e2 = tr.on_report(&cfg, t(2_000), canon(102.0), 101.75);
        assert_eq!(e2.correction, None);
        let e3 = tr.on_report(&cfg, t(11_000), canon(111.0), 110.7);
        assert!(matches!(e3.correction, Some(Correction::AdjustRate { .. })));
    }

    #[test]
    fn rtt_is_smoothed_and_clamped() {
        let mut tr = DriftTracker::default();
        tr.record_rtt(100.0);
        tr.record_rtt(200.0);
        assert!((tr.rtt_ms().unwrap() - 130.0).abs() < 1e-9);
        tr.record_rtt(f64::NAN);
        tr.record_rtt(1e12);
        assert!(tr.rtt_ms().unwrap() <= 10_000.0);
    }
}

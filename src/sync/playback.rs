//! Canonical playback state of a room.
//!
//! The state is `(position, status, rate)` anchored at a reference instant.
//! Nobody streams the position: while playing, anyone can derive it with
//! `position + elapsed * rate`. Every change bumps a monotonic `sequence`.

use serde::{Deserialize, Serialize};

use super::clock::{Millis, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
}

/// Wire representation of the playback state, rebased at `server_time`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    #[serde(rename = "state")]
    pub status: PlaybackStatus,
    /// Position in seconds at `server_time`.
    pub position: f64,
    pub rate: f64,
    /// Server wall clock (ms since Unix epoch) at which `position` is valid.
    pub server_time: Millis,
    /// Room-wide, strictly increasing change counter.
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct PlaybackState {
    status: PlaybackStatus,
    /// Position in seconds at `reference_mono`.
    position: f64,
    rate: f64,
    reference_mono: Millis,
    sequence: u64,
    duration: Option<f64>,
}

impl PlaybackState {
    pub fn new(now: Time) -> Self {
        Self {
            status: PlaybackStatus::Paused,
            position: 0.0,
            rate: 1.0,
            reference_mono: now.mono_ms,
            sequence: 0,
            duration: None,
        }
    }

    pub fn status(&self) -> PlaybackStatus {
        self.status
    }

    pub fn is_playing(&self) -> bool {
        self.status == PlaybackStatus::Playing
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Canonical position at `now`, computed from the monotonic clock only.
    pub fn position_at(&self, now: Time) -> f64 {
        let pos = match self.status {
            PlaybackStatus::Paused => self.position,
            PlaybackStatus::Playing => {
                let elapsed_ms = now.mono_ms.saturating_sub(self.reference_mono);
                self.position + (elapsed_ms as f64 / 1000.0) * self.rate
            }
        };
        match self.duration {
            Some(d) => pos.min(d),
            None => pos,
        }
    }

    pub fn snapshot(&self, now: Time) -> PlaybackSnapshot {
        PlaybackSnapshot {
            status: self.status,
            position: self.position_at(now),
            rate: self.rate,
            server_time: now.wall_ms,
            sequence: self.sequence,
        }
    }

    /// New media: paused at 0, rate preserved.
    pub fn reset_for_media(&mut self, duration: Option<f64>, now: Time) {
        self.status = PlaybackStatus::Paused;
        self.position = 0.0;
        self.duration = duration;
        self.reference_mono = now.mono_ms;
        self.bump();
    }

    /// Start/resume playback, optionally from `position`. Returns false when nothing changed.
    pub fn play(&mut self, position: Option<f64>, now: Time) -> bool {
        if self.is_playing() && position.is_none() {
            return false;
        }
        let pos = position.unwrap_or_else(|| self.position_at(now));
        self.rebase(pos, now);
        self.status = PlaybackStatus::Playing;
        self.bump();
        true
    }

    /// Pause, optionally at an explicit `position`. Returns false when nothing changed.
    pub fn pause(&mut self, position: Option<f64>, now: Time) -> bool {
        if !self.is_playing() && position.is_none() {
            return false;
        }
        let pos = position.unwrap_or_else(|| self.position_at(now));
        self.rebase(pos, now);
        self.status = PlaybackStatus::Paused;
        self.bump();
        true
    }

    pub fn seek(&mut self, position: f64, now: Time) {
        self.rebase(position, now);
        self.bump();
    }

    /// Change the rate without moving the position. Returns false when unchanged.
    pub fn set_rate(&mut self, rate: f64, now: Time) -> bool {
        if (rate - self.rate).abs() < f64::EPSILON {
            return false;
        }
        let pos = self.position_at(now);
        self.rebase(pos, now);
        self.rate = rate;
        self.bump();
        true
    }

    fn rebase(&mut self, position: f64, now: Time) {
        self.position = position;
        self.reference_mono = now.mono_ms;
    }

    fn bump(&mut self) {
        self.sequence += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ms: u64) -> Time {
        Time {
            mono_ms: ms,
            wall_ms: 1_700_000_000_000 + ms,
        }
    }

    #[test]
    fn starts_paused_at_zero() {
        let s = PlaybackState::new(t(0));
        assert_eq!(s.status(), PlaybackStatus::Paused);
        assert_eq!(s.position_at(t(10_000)), 0.0);
        assert_eq!(s.sequence(), 0);
    }

    #[test]
    fn position_advances_with_rate_only_while_playing() {
        let mut s = PlaybackState::new(t(0));
        s.play(Some(100.0), t(0));
        assert!((s.position_at(t(2_000)) - 102.0).abs() < 1e-9);
        s.set_rate(1.5, t(2_000));
        assert!((s.position_at(t(4_000)) - 105.0).abs() < 1e-9);
        s.pause(None, t(4_000));
        assert!((s.position_at(t(60_000)) - 105.0).abs() < 1e-9);
    }

    #[test]
    fn sequence_is_monotonic_and_idempotent_ops_do_not_bump() {
        let mut s = PlaybackState::new(t(0));
        s.reset_for_media(None, t(0));
        assert_eq!(s.sequence(), 1);
        assert!(s.play(None, t(10)));
        assert_eq!(s.sequence(), 2);
        assert!(!s.play(None, t(20)), "already playing");
        assert_eq!(s.sequence(), 2);
        s.seek(50.0, t(30));
        assert_eq!(s.sequence(), 3);
        assert!(s.pause(None, t(40)));
        assert!(!s.pause(None, t(50)), "already paused");
        assert!(!s.set_rate(1.0, t(60)), "same rate");
        assert_eq!(s.sequence(), 4);
    }

    #[test]
    fn seek_while_playing_keeps_playing_from_new_position() {
        let mut s = PlaybackState::new(t(0));
        s.play(Some(10.0), t(0));
        s.seek(500.0, t(1_000));
        assert_eq!(s.status(), PlaybackStatus::Playing);
        assert!((s.position_at(t(3_000)) - 502.0).abs() < 1e-9);
    }

    #[test]
    fn snapshot_is_rebased_at_server_time() {
        let mut s = PlaybackState::new(t(0));
        s.play(Some(10.0), t(0));
        let snap = s.snapshot(t(1_500));
        assert!((snap.position - 11.5).abs() < 1e-9);
        assert_eq!(snap.server_time, 1_700_000_000_000 + 1_500);
        assert_eq!(snap.status, PlaybackStatus::Playing);
    }

    #[test]
    fn position_is_clamped_to_duration() {
        let mut s = PlaybackState::new(t(0));
        s.reset_for_media(Some(60.0), t(0));
        s.play(Some(55.0), t(0));
        assert_eq!(s.position_at(t(100_000)), 60.0);
    }

    #[test]
    fn media_change_resets_position_and_pauses() {
        let mut s = PlaybackState::new(t(0));
        s.play(Some(300.0), t(0));
        s.reset_for_media(None, t(5_000));
        assert_eq!(s.status(), PlaybackStatus::Paused);
        assert_eq!(s.position_at(t(9_000)), 0.0);
    }

    #[test]
    fn clock_going_backwards_never_underflows() {
        let mut s = PlaybackState::new(t(10_000));
        s.play(Some(5.0), t(10_000));
        assert_eq!(s.position_at(t(9_000)), 5.0);
    }
}

//! Synchronization engine: time, canonical playback state, drift policy.
//!
//! This module is pure (no I/O, no async, no web framework) so it can be
//! tested deterministically.

pub mod clock;
pub mod drift;
pub mod playback;

pub use clock::{Clock, ManualClock, Millis, SystemClock, Time};
pub use drift::{Correction, DriftConfig, DriftTracker};
pub use playback::{PlaybackSnapshot, PlaybackState, PlaybackStatus};

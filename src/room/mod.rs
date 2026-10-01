//! Room domain: one watch session.
//!
//! `Room` is a synchronous state machine. It never touches sockets, tasks or
//! the system clock: callers pass the current [`Time`] and receive an
//! [`Outcome`] describing which messages to deliver to whom. That keeps the
//! synchronization engine independent from Axum/WebSockets and testable
//! deterministically. [`manager::RoomManager`] is the (only) adapter that
//! owns rooms, locks and delivery channels.

pub mod manager;
#[allow(clippy::module_inception)]
mod room;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::chat::ChatConfig;
use crate::protocol::{ControlMode, ServerMessage};
use crate::sync::DriftConfig;

pub use manager::{Attachment, CloseReason, CreatedRoom, Outbound, RoomManager};
pub use room::Room;

/// What happens to a room when its host leaves for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostLeavePolicy {
    /// Ownership moves to the longest-standing remaining participant (preferring connected ones).
    Transfer,
    /// The room is closed for everybody.
    Close,
}

#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub max_participants: usize,
    pub host_leave_policy: HostLeavePolicy,
    pub default_control_mode: ControlMode,
    pub chat_enabled: bool,
    /// How long a dropped participant keeps their seat.
    pub reconnect_grace_ms: u64,
    /// How long a participant who joined over HTTP has to open their socket.
    pub connect_grace_ms: u64,
    /// How long an empty room lingers before destruction.
    pub empty_timeout_ms: u64,
    /// A room with no inbound activity for this long expires.
    pub idle_timeout_ms: u64,
    /// Interval of the `sync_state` heartbeat while playing.
    pub heartbeat_interval_ms: u64,
    pub rate_min: f64,
    pub rate_max: f64,
    pub max_position_secs: f64,
    /// Per-participant inbound message rate limit.
    pub msg_rate_per_sec: f64,
    pub msg_burst: f64,
    pub drift: DriftConfig,
    pub chat: ChatConfig,
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            max_participants: 100,
            host_leave_policy: HostLeavePolicy::Transfer,
            default_control_mode: ControlMode::Everyone,
            chat_enabled: true,
            reconnect_grace_ms: 30_000,
            connect_grace_ms: 60_000,
            empty_timeout_ms: 60_000,
            idle_timeout_ms: 12 * 3600 * 1000,
            heartbeat_interval_ms: 10_000,
            rate_min: 0.25,
            rate_max: 4.0,
            max_position_secs: 7.0 * 24.0 * 3600.0,
            msg_rate_per_sec: 20.0,
            msg_burst: 40.0,
            drift: DriftConfig::default(),
            chat: ChatConfig::default(),
        }
    }
}

/// Identity of a participant as far as a room is concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantInfo {
    pub id: String,
    pub display_name: String,
    pub can_chat: bool,
}

/// Who a message is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    All,
    AllExcept(String),
    Only(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub target: Target,
    pub message: ServerMessage,
}

/// Result of a room operation.
#[derive(Debug, Default)]
pub struct Outcome {
    pub deliveries: Vec<Delivery>,
    /// Participants that are no longer in the room; their connections must be closed
    /// after the deliveries have been sent.
    pub removed: Vec<String>,
    /// The room is over and must be destroyed.
    pub closed: bool,
    /// Number of drift corrections issued (metrics).
    pub corrections: u32,
}

impl Outcome {
    pub fn all(&mut self, message: ServerMessage) {
        self.deliveries.push(Delivery {
            target: Target::All,
            message,
        });
    }

    pub fn except(&mut self, pid: &str, message: ServerMessage) {
        self.deliveries.push(Delivery {
            target: Target::AllExcept(pid.to_owned()),
            message,
        });
    }

    pub fn only(&mut self, pid: &str, message: ServerMessage) {
        self.deliveries.push(Delivery {
            target: Target::Only(pid.to_owned()),
            message,
        });
    }
}

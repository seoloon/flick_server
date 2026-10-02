//! Versioned WebSocket protocol (JSON).
//!
//! Every frame is `{"protocol_version":1,"type":"...","payload":{...}}`.
//! This module only defines wire types and parsing: it knows nothing about
//! Axum or sockets, so the room logic can use it directly.

pub mod media;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat::ChatMessage;
use crate::errors::{Error, ErrorCode, Result};
use crate::sync::clock::Millis;
use crate::sync::playback::PlaybackSnapshot;

pub use media::{MediaRef, MediaType, Provider};

pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------- shared enums

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// Every participant can play/pause/seek/change rate.
    Everyone,
    /// Only the host can control playback.
    HostOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Connected,
    /// The socket dropped; the participant is within the reconnection grace period.
    Reconnecting,
    /// Joined but never connected a socket (yet).
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomLifecycle {
    Waiting,
    MediaSelected,
    Playing,
    Paused,
    Empty,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaveReason {
    Left,
    /// Reconnection grace period elapsed.
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    HostClosed,
    HostLeft,
    Expired,
    Empty,
    Shutdown,
    /// Closed by the server operator (web panel / admin API).
    AdminClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomUpdateReason {
    HostChanged,
    SettingsChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncReason {
    Heartbeat,
    Requested,
    /// The client reported a stale sequence or a mismatching play state.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionAction {
    AdjustRate,
    Seek,
}

// ---------------------------------------------------------------- server -> client

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticipantView {
    pub participant_id: String,
    pub display_name: String,
    pub presence: Presence,
    pub is_host: bool,
    pub joined_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomView {
    pub room_id: String,
    pub state: RoomLifecycle,
    pub host_id: String,
    pub control_mode: ControlMode,
    pub chat_enabled: bool,
    pub max_participants: usize,
    pub participants: Vec<ParticipantView>,
    pub media: Option<MediaRef>,
    pub playback: PlaybackSnapshot,
    pub created_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaybackEvent {
    /// Participant whose command caused this change.
    pub by: String,
    #[serde(flatten)]
    pub playback: PlaybackSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncStatePayload {
    pub reason: SyncReason,
    #[serde(flatten)]
    pub playback: PlaybackSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncCorrectionPayload {
    pub action: CorrectionAction,
    /// `adjust_rate`: the temporary playback rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// `adjust_rate`: how long to keep the temporary rate before returning to the room rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// `seek`: target position, already compensated for network latency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    /// Measured drift (positive = this client is ahead).
    pub drift_ms: f64,
    pub sequence: u64,
    pub server_time: Millis,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomState {
        room: RoomView,
        /// Your participant id.
        you: String,
        server_time: Millis,
    },
    ParticipantJoined {
        participant: ParticipantView,
    },
    ParticipantLeft {
        participant_id: String,
        reason: LeaveReason,
    },
    PresenceChanged {
        participant_id: String,
        presence: Presence,
    },
    MediaSelected {
        media: MediaRef,
        playback: PlaybackSnapshot,
        by: String,
    },
    PlaybackPlay(PlaybackEvent),
    PlaybackPause(PlaybackEvent),
    PlaybackSeek(PlaybackEvent),
    PlaybackRateChanged(PlaybackEvent),
    SyncState(SyncStatePayload),
    SyncCorrection(SyncCorrectionPayload),
    RoomUpdated {
        host_id: String,
        control_mode: ControlMode,
        chat_enabled: bool,
        state: RoomLifecycle,
        reason: RoomUpdateReason,
    },
    RoomClosed {
        reason: ClosedReason,
    },
    ChatMessage(ChatMessage),
    ChatHistory {
        messages: Vec<ChatMessage>,
    },
    Pong {
        /// Echo of the client's `client_time`.
        client_time: f64,
        /// Server wall clock (ms since Unix epoch) when the ping was handled.
        server_time: Millis,
    },
    Error(ErrorPayload),
}

#[derive(Serialize)]
struct OutEnvelope<'a> {
    protocol_version: u32,
    #[serde(flatten)]
    message: &'a ServerMessage,
}

impl ServerMessage {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        ServerMessage::Error(ErrorPayload {
            code,
            message: message.into(),
        })
    }

    pub fn from_error(e: &Error) -> Self {
        Self::error(e.code, e.message.clone())
    }

    /// Serialize into a full wire frame.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&OutEnvelope {
            protocol_version: PROTOCOL_VERSION,
            message: self,
        })
        // Serialization of these plain data types cannot fail; never panic regardless.
        .unwrap_or_else(|_| {
            r#"{"protocol_version":1,"type":"error","payload":{"code":"INTERNAL","message":"serialization failed"}}"#
                .to_owned()
        })
    }
}

// ---------------------------------------------------------------- client -> server

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PingPayload {
    /// Opaque client timestamp, echoed back in `pong`.
    pub client_time: f64,
    /// Last RTT the client measured (ms). Used for latency compensation.
    #[serde(default)]
    pub rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SelectMediaPayload {
    pub media: MediaRef,
}

/// Shared by play / pause / seek.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct PositionPayload {
    #[serde(default)]
    pub position: Option<f64>,
    /// Last room `sequence` the client has seen. Must not be ahead of the server.
    #[serde(default)]
    pub sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RatePayload {
    pub rate: f64,
    #[serde(default)]
    pub sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SyncReportPayload {
    /// Position (s) the client is playing at right now.
    pub position: f64,
    /// Last room `sequence` applied by the client.
    #[serde(default)]
    pub sequence: Option<u64>,
    /// Local state; a mismatch with the room triggers a `sync_state` instead of a correction.
    #[serde(default)]
    pub state: Option<crate::sync::playback::PlaybackStatus>,
    /// True while the client is buffering (drift is not evaluated).
    #[serde(default)]
    pub buffering: bool,
    #[serde(default)]
    pub rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ChatSendPayload {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct UpdateRoomPayload {
    #[serde(default)]
    pub control_mode: Option<ControlMode>,
    #[serde(default)]
    pub chat_enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Ping(PingPayload),
    SelectMedia(SelectMediaPayload),
    PlaybackPlay(PositionPayload),
    PlaybackPause(PositionPayload),
    PlaybackSeek(PositionPayload),
    PlaybackRateChanged(RatePayload),
    SyncRequest,
    SyncReport(SyncReportPayload),
    ChatMessage(ChatSendPayload),
    UpdateRoom(UpdateRoomPayload),
    CloseRoom,
    LeaveRoom,
}

#[derive(Deserialize)]
struct InEnvelope {
    #[serde(default)]
    protocol_version: Option<u32>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Value,
}

fn payload<T: serde::de::DeserializeOwned>(kind: &str, v: Value) -> Result<T> {
    // A missing/null payload is treated as an empty object.
    let v = if v.is_null() {
        Value::Object(Default::default())
    } else {
        v
    };
    if !v.is_object() {
        return Err(Error::new(
            ErrorCode::InvalidPayload,
            format!("payload for {kind} must be a JSON object"),
        ));
    }
    serde_json::from_value(v).map_err(|e| {
        Error::new(
            ErrorCode::InvalidPayload,
            format!("invalid payload for {kind}: {e}"),
        )
    })
}

impl ClientMessage {
    /// Parse a text frame. Never panics; every failure is a typed protocol error.
    pub fn parse(text: &str) -> Result<Self> {
        let env: InEnvelope = serde_json::from_str(text).map_err(|_| {
            Error::new(
                ErrorCode::InvalidMessage,
                "frame is not a valid message envelope",
            )
        })?;
        if let Some(v) = env.protocol_version
            && v != PROTOCOL_VERSION
        {
            return Err(Error::new(
                ErrorCode::UnsupportedVersion,
                format!("unsupported protocol_version {v}; this server speaks {PROTOCOL_VERSION}"),
            ));
        }
        let k = env.kind.as_str();
        let p = env.payload;
        Ok(match k {
            "ping" => Self::Ping(payload(k, p)?),
            "select_media" => Self::SelectMedia(payload(k, p)?),
            "playback_play" => Self::PlaybackPlay(payload(k, p)?),
            "playback_pause" => Self::PlaybackPause(payload(k, p)?),
            "playback_seek" => Self::PlaybackSeek(payload(k, p)?),
            "playback_rate_changed" => Self::PlaybackRateChanged(payload(k, p)?),
            "sync_request" => Self::SyncRequest,
            "sync_report" => Self::SyncReport(payload(k, p)?),
            "chat_message" => Self::ChatMessage(payload(k, p)?),
            "update_room" => Self::UpdateRoom(payload(k, p)?),
            "close_room" => Self::CloseRoom,
            "leave_room" => Self::LeaveRoom,
            other => {
                // Do not echo unbounded client input back.
                let shown: String = other.chars().take(64).collect();
                return Err(Error::new(
                    ErrorCode::UnknownType,
                    format!("unknown message type '{shown}'"),
                ));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::playback::PlaybackStatus;

    #[test]
    fn parses_spec_example_pause() {
        let m = ClientMessage::parse(r#"{"type":"playback_pause","payload":{"position":123.42}}"#)
            .unwrap();
        assert_eq!(
            m,
            ClientMessage::PlaybackPause(PositionPayload {
                position: Some(123.42),
                sequence: None
            })
        );
    }

    #[test]
    fn missing_payload_defaults_to_empty() {
        assert!(matches!(
            ClientMessage::parse(r#"{"type":"playback_play"}"#).unwrap(),
            ClientMessage::PlaybackPlay(_)
        ));
        assert_eq!(
            ClientMessage::parse(r#"{"type":"sync_request"}"#).unwrap(),
            ClientMessage::SyncRequest
        );
    }

    #[test]
    fn malformed_inputs_yield_typed_errors() {
        let cases = [
            ("not json", ErrorCode::InvalidMessage),
            ("", ErrorCode::InvalidMessage),
            ("[]", ErrorCode::InvalidMessage),
            (r#"{"payload":{}}"#, ErrorCode::InvalidMessage),
            (r#"{"type":5}"#, ErrorCode::InvalidMessage),
            (r#"{"type":"nope","payload":{}}"#, ErrorCode::UnknownType),
            (
                r#"{"type":"playback_seek","payload":{"position":"x"}}"#,
                ErrorCode::InvalidPayload,
            ),
            (
                r#"{"type":"playback_seek","payload":[1]}"#,
                ErrorCode::InvalidPayload,
            ),
            (
                r#"{"type":"playback_rate_changed","payload":{}}"#,
                ErrorCode::InvalidPayload,
            ),
            (
                r#"{"type":"playback_seek","payload":{"sequence":-1}}"#,
                ErrorCode::InvalidPayload,
            ),
            (
                r#"{"protocol_version":2,"type":"ping","payload":{"client_time":1}}"#,
                ErrorCode::UnsupportedVersion,
            ),
            (
                r#"{"type":"select_media","payload":{"media":{"provider":"x"}}}"#,
                ErrorCode::InvalidPayload,
            ),
        ];
        for (input, code) in cases {
            assert_eq!(
                ClientMessage::parse(input).unwrap_err().code,
                code,
                "{input}"
            );
        }
    }

    #[test]
    fn unknown_type_error_truncates_echo() {
        let long = "x".repeat(10_000);
        let err = ClientMessage::parse(&format!(r#"{{"type":"{long}"}}"#)).unwrap_err();
        assert!(err.message.len() < 200);
    }

    #[test]
    fn unknown_extra_fields_are_ignored_for_forward_compat() {
        let m = ClientMessage::parse(
            r#"{"protocol_version":1,"type":"playback_seek","payload":{"position":5,"future":true}}"#,
        )
        .unwrap();
        assert!(matches!(m, ClientMessage::PlaybackSeek(_)));
    }

    #[test]
    fn sync_report_parses_state() {
        let m = ClientMessage::parse(
            r#"{"type":"sync_report","payload":{"position":10.5,"state":"playing","sequence":3}}"#,
        )
        .unwrap();
        match m {
            ClientMessage::SyncReport(r) => {
                assert_eq!(r.state, Some(PlaybackStatus::Playing));
                assert!(!r.buffering);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn server_messages_use_envelope_and_flattened_playback() {
        let msg = ServerMessage::PlaybackPause(PlaybackEvent {
            by: "alice".into(),
            playback: PlaybackSnapshot {
                status: PlaybackStatus::Paused,
                position: 123.42,
                rate: 1.0,
                server_time: 1234567890,
                sequence: 42,
            },
        });
        let v: Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(v["protocol_version"], 1);
        assert_eq!(v["type"], "playback_pause");
        assert_eq!(v["payload"]["by"], "alice");
        assert_eq!(v["payload"]["state"], "paused");
        assert_eq!(v["payload"]["position"], 123.42);
        assert_eq!(v["payload"]["server_time"], 1234567890u64);
        assert_eq!(v["payload"]["sequence"], 42);
    }

    #[test]
    fn error_message_shape_matches_spec() {
        let v: Value = serde_json::from_str(
            &ServerMessage::error(ErrorCode::NotHost, "Only the room host can select media.")
                .to_json(),
        )
        .unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["payload"]["code"], "NOT_HOST");
    }
}

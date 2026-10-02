//! The room state machine.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info};

use super::{
    AdminParticipant, AdminRoomView, HostLeavePolicy, Outcome, ParticipantInfo, RoomConfig,
};
use crate::chat::ChatLog;
use crate::errors::{Error, ErrorCode, Result};
use crate::protocol::{
    ClientMessage, ClosedReason, ControlMode, CorrectionAction, LeaveReason, MediaRef,
    ParticipantView, PlaybackEvent, Presence, RoomLifecycle, RoomUpdateReason, RoomView,
    ServerMessage, SyncCorrectionPayload, SyncReason, SyncReportPayload, SyncStatePayload,
    UpdateRoomPayload,
};
use crate::ratelimit::TokenBucket;
use crate::sync::clock::{Millis, Time};
use crate::sync::drift::{Canonical, Correction, DriftTracker};
use crate::sync::playback::PlaybackState;

/// Serialized name of a plain enum (`snake_case` wire form).
fn enum_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

struct Participant {
    info: ParticipantInfo,
    presence: Presence,
    /// Arrival order, used to pick the next host.
    join_order: u64,
    joined_at: Millis,
    /// Monotonic instant at which the participant stopped being connected.
    away_since: Option<Millis>,
    tracker: DriftTracker,
    msg_bucket: TokenBucket,
    chat_bucket: TokenBucket,
}

pub struct Room {
    id: String,
    server_id: String,
    host: String,
    participants: HashMap<String, Participant>,
    next_join_order: u64,
    media: Option<MediaRef>,
    /// False between media selection and the first `play`.
    started: bool,
    playback: PlaybackState,
    control_mode: ControlMode,
    chat_enabled: bool,
    chat: ChatLog,
    created_at: Millis,
    last_activity: Millis,
    empty_since: Option<Millis>,
    last_heartbeat: Millis,
    closed: bool,
    cfg: Arc<RoomConfig>,
}

impl Room {
    /// Create a room. The host is registered as a participant that has not connected yet.
    pub fn new(
        id: String,
        server_id: String,
        host: ParticipantInfo,
        control_mode: ControlMode,
        chat_enabled: bool,
        cfg: Arc<RoomConfig>,
        now: Time,
    ) -> Self {
        let host_id = host.id.clone();
        let mut room = Self {
            id,
            server_id,
            host: host_id,
            participants: HashMap::new(),
            next_join_order: 0,
            media: None,
            started: false,
            playback: PlaybackState::new(now),
            control_mode,
            chat_enabled,
            chat: ChatLog::new(),
            created_at: now.wall_ms,
            last_activity: now.mono_ms,
            empty_since: None,
            last_heartbeat: now.mono_ms,
            closed: false,
            cfg,
        };
        room.insert_participant(host, now);
        room
    }

    // ------------------------------------------------------------ accessors

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn host_id(&self) -> &str {
        &self.host
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn has_participant(&self, pid: &str) -> bool {
        self.participants.contains_key(pid)
    }

    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }

    pub fn connected_count(&self) -> usize {
        self.participants
            .values()
            .filter(|p| p.presence == Presence::Connected)
            .count()
    }

    pub fn is_full(&self) -> bool {
        self.participants.len() >= self.cfg.max_participants
    }

    pub fn presence_of(&self, pid: &str) -> Option<Presence> {
        self.participants.get(pid).map(|p| p.presence)
    }

    pub fn sequence(&self) -> u64 {
        self.playback.sequence()
    }

    pub fn lifecycle(&self) -> RoomLifecycle {
        if self.closed {
            RoomLifecycle::Closed
        } else if self.participants.is_empty() {
            RoomLifecycle::Empty
        } else if self.media.is_none() {
            RoomLifecycle::Waiting
        } else if !self.started {
            RoomLifecycle::MediaSelected
        } else if self.playback.is_playing() {
            RoomLifecycle::Playing
        } else {
            RoomLifecycle::Paused
        }
    }

    /// Sum and count of smoothed RTTs (metrics).
    pub fn rtt_stats(&self) -> (f64, u32) {
        self.participants
            .values()
            .filter_map(|p| p.tracker.rtt_ms())
            .fold((0.0, 0), |(s, n), r| (s + r, n + 1))
    }

    pub fn view(&self, now: Time) -> RoomView {
        let mut participants: Vec<(u64, ParticipantView)> = self
            .participants
            .values()
            .map(|p| {
                (
                    p.join_order,
                    ParticipantView {
                        participant_id: p.info.id.clone(),
                        display_name: p.info.display_name.clone(),
                        presence: p.presence,
                        is_host: p.info.id == self.host,
                        joined_at: p.joined_at,
                    },
                )
            })
            .collect();
        participants.sort_by_key(|(order, _)| *order);
        RoomView {
            room_id: self.id.clone(),
            state: self.lifecycle(),
            host_id: self.host.clone(),
            control_mode: self.control_mode,
            chat_enabled: self.chat_enabled,
            max_participants: self.cfg.max_participants,
            participants: participants.into_iter().map(|(_, v)| v).collect(),
            media: self.media.clone(),
            playback: self.playback.snapshot(now),
            created_at: self.created_at,
        }
    }

    /// Operator view (admin API).
    pub fn admin_view(&self, now: Time) -> AdminRoomView {
        let mut ps: Vec<(u64, AdminParticipant)> = self
            .participants
            .values()
            .map(|p| {
                (
                    p.join_order,
                    AdminParticipant {
                        participant_id: p.info.id.clone(),
                        display_name: p.info.display_name.clone(),
                        presence: p.presence,
                        is_host: p.info.id == self.host,
                        joined_at: p.joined_at,
                        rtt_ms: p.tracker.rtt_ms(),
                    },
                )
            })
            .collect();
        ps.sort_by_key(|(order, _)| *order);
        AdminRoomView {
            room_id: self.id.clone(),
            state: self.lifecycle(),
            host_id: self.host.clone(),
            control_mode: self.control_mode,
            chat_enabled: self.chat_enabled,
            max_participants: self.cfg.max_participants,
            participants: ps.into_iter().map(|(_, p)| p).collect(),
            media_title: self.media.as_ref().and_then(|m| m.title.clone()),
            media_provider: self.media.as_ref().map(|m| enum_name(&m.provider)),
            media_type: self.media.as_ref().map(|m| enum_name(&m.media_type)),
            playback: self.playback.snapshot(now),
            created_at: self.created_at,
            age_secs: now.wall_ms.saturating_sub(self.created_at) / 1000,
            idle_secs: now.mono_ms.saturating_sub(self.last_activity) / 1000,
        }
    }

    /// Close the room on behalf of the operator.
    pub fn admin_close(&mut self) -> Outcome {
        let mut out = Outcome::default();
        self.close(ClosedReason::AdminClosed, &mut out);
        out
    }

    // ------------------------------------------------------------ membership

    fn insert_participant(&mut self, info: ParticipantInfo, now: Time) {
        let order = self.next_join_order;
        self.next_join_order += 1;
        let cfg = &self.cfg;
        let p = Participant {
            presence: Presence::Disconnected,
            join_order: order,
            joined_at: now.wall_ms,
            away_since: Some(now.mono_ms),
            tracker: DriftTracker::default(),
            msg_bucket: TokenBucket::new(cfg.msg_burst, cfg.msg_rate_per_sec, now.mono_ms),
            chat_bucket: TokenBucket::new(cfg.chat.burst, cfg.chat.rate_per_sec, now.mono_ms),
            info,
        };
        self.participants.insert(p.info.id.clone(), p);
    }

    /// Register a participant (idempotent). They are `Disconnected` until [`Room::connect`].
    pub fn join(&mut self, info: ParticipantInfo, now: Time) -> Result<Outcome> {
        if self.closed {
            return Err(Error::new(ErrorCode::RoomClosed, "this room is closed"));
        }
        let mut out = Outcome::default();
        if let Some(existing) = self.participants.get_mut(&info.id) {
            existing.info = info;
            return Ok(out);
        }
        if self.is_full() {
            return Err(Error::new(ErrorCode::RoomFull, "this room is full"));
        }
        let view_id = info.id.clone();
        self.insert_participant(info, now);
        self.empty_since = None;
        self.last_activity = now.mono_ms;
        info!(room_id = %self.id, participant_id = %view_id, "participant joined");
        if let Some(p) = self.participants.get(&view_id) {
            let participant = self.participant_view(p);
            out.except(&view_id, ServerMessage::ParticipantJoined { participant });
        }
        Ok(out)
    }

    fn participant_view(&self, p: &Participant) -> ParticipantView {
        ParticipantView {
            participant_id: p.info.id.clone(),
            display_name: p.info.display_name.clone(),
            presence: p.presence,
            is_host: p.info.id == self.host,
            joined_at: p.joined_at,
        }
    }

    /// A socket is now attached for `pid`: mark connected and send the full state.
    pub fn connect(&mut self, pid: &str, now: Time) -> Result<Outcome> {
        if self.closed {
            return Err(Error::new(ErrorCode::RoomClosed, "this room is closed"));
        }
        let p = self
            .participants
            .get_mut(pid)
            .ok_or_else(|| Error::new(ErrorCode::NotMember, "not a member of this room"))?;
        let changed = p.presence != Presence::Connected;
        p.presence = Presence::Connected;
        p.away_since = None;
        self.last_activity = now.mono_ms;

        let mut out = Outcome::default();
        if changed {
            out.except(
                pid,
                ServerMessage::PresenceChanged {
                    participant_id: pid.to_owned(),
                    presence: Presence::Connected,
                },
            );
        }
        out.only(
            pid,
            ServerMessage::RoomState {
                room: self.view(now),
                you: pid.to_owned(),
                server_time: now.wall_ms,
            },
        );
        if self.chat_enabled {
            out.only(
                pid,
                ServerMessage::ChatHistory {
                    messages: self.chat.history(),
                },
            );
        }
        Ok(out)
    }

    /// The socket of `pid` went away: keep the seat for the grace period.
    pub fn disconnect(&mut self, pid: &str, now: Time) -> Outcome {
        let mut out = Outcome::default();
        if let Some(p) = self.participants.get_mut(pid)
            && p.presence == Presence::Connected
        {
            p.presence = Presence::Reconnecting;
            p.away_since = Some(now.mono_ms);
            debug!(room_id = %self.id, participant_id = %pid, "participant disconnected");
            out.except(
                pid,
                ServerMessage::PresenceChanged {
                    participant_id: pid.to_owned(),
                    presence: Presence::Reconnecting,
                },
            );
        }
        out
    }

    /// Permanent departure.
    pub fn leave(&mut self, pid: &str, now: Time) -> Result<Outcome> {
        if !self.participants.contains_key(pid) {
            return Err(Error::new(
                ErrorCode::NotMember,
                "not a member of this room",
            ));
        }
        let mut out = Outcome::default();
        self.remove_participant(pid, LeaveReason::Left, now, &mut out);
        Ok(out)
    }

    fn remove_participant(&mut self, pid: &str, reason: LeaveReason, now: Time, out: &mut Outcome) {
        if self.participants.remove(pid).is_none() {
            return;
        }
        info!(room_id = %self.id, participant_id = %pid, ?reason, "participant left");
        out.removed.push(pid.to_owned());
        out.except(
            pid,
            ServerMessage::ParticipantLeft {
                participant_id: pid.to_owned(),
                reason,
            },
        );
        if self.participants.is_empty() {
            self.empty_since = Some(now.mono_ms);
            return;
        }
        if pid != self.host {
            return;
        }
        match self.cfg.host_leave_policy {
            HostLeavePolicy::Close => self.close(ClosedReason::HostLeft, out),
            HostLeavePolicy::Transfer => {
                let next = self
                    .participants
                    .values()
                    .min_by_key(|p| (p.presence != Presence::Connected, p.join_order))
                    .map(|p| p.info.id.clone());
                if let Some(next) = next {
                    info!(room_id = %self.id, old_host = %pid, new_host = %next, "host transferred");
                    self.host = next;
                    out.all(self.room_updated(RoomUpdateReason::HostChanged, now));
                }
            }
        }
    }

    fn room_updated(&self, reason: RoomUpdateReason, _now: Time) -> ServerMessage {
        ServerMessage::RoomUpdated {
            host_id: self.host.clone(),
            control_mode: self.control_mode,
            chat_enabled: self.chat_enabled,
            state: self.lifecycle(),
            reason,
        }
    }

    fn close(&mut self, reason: ClosedReason, out: &mut Outcome) {
        if self.closed {
            return;
        }
        self.closed = true;
        out.closed = true;
        info!(room_id = %self.id, ?reason, "room closed");
        out.all(ServerMessage::RoomClosed { reason });
    }

    /// Close the room because the server is shutting down.
    pub fn shutdown(&mut self) -> Outcome {
        let mut out = Outcome::default();
        self.close(ClosedReason::Shutdown, &mut out);
        out
    }

    // ------------------------------------------------------------ time-driven work

    /// Periodic maintenance: grace expiry, empty/idle expiry, sync heartbeat.
    pub fn tick(&mut self, now: Time) -> Outcome {
        let mut out = Outcome::default();
        if self.closed {
            out.closed = true;
            return out;
        }

        let expired: Vec<String> = self
            .participants
            .values()
            .filter(|p| {
                let grace = match p.presence {
                    Presence::Connected => return false,
                    Presence::Reconnecting => self.cfg.reconnect_grace_ms,
                    Presence::Disconnected => self.cfg.connect_grace_ms,
                };
                p.away_since
                    .is_some_and(|t| now.mono_ms.saturating_sub(t) >= grace)
            })
            .map(|p| p.info.id.clone())
            .collect();
        for pid in expired {
            self.remove_participant(&pid, LeaveReason::Timeout, now, &mut out);
            if self.closed {
                return out;
            }
        }

        if self.participants.is_empty() {
            let since = *self.empty_since.get_or_insert(now.mono_ms);
            if now.mono_ms.saturating_sub(since) >= self.cfg.empty_timeout_ms {
                self.close(ClosedReason::Empty, &mut out);
            }
            return out;
        }

        if now.mono_ms.saturating_sub(self.last_activity) >= self.cfg.idle_timeout_ms {
            self.close(ClosedReason::Expired, &mut out);
            return out;
        }

        if self.playback.is_playing()
            && self.connected_count() > 0
            && now.mono_ms.saturating_sub(self.last_heartbeat) >= self.cfg.heartbeat_interval_ms
        {
            self.last_heartbeat = now.mono_ms;
            out.all(self.sync_state(SyncReason::Heartbeat, now));
        }
        out
    }

    fn sync_state(&self, reason: SyncReason, now: Time) -> ServerMessage {
        ServerMessage::SyncState(SyncStatePayload {
            reason,
            playback: self.playback.snapshot(now),
        })
    }

    // ------------------------------------------------------------ commands

    /// Validate and apply a client command from `pid`.
    pub fn handle(&mut self, pid: &str, msg: ClientMessage, now: Time) -> Result<Outcome> {
        if self.closed {
            return Err(Error::new(ErrorCode::RoomClosed, "this room is closed"));
        }
        let p = self
            .participants
            .get_mut(pid)
            .ok_or_else(|| Error::new(ErrorCode::NotMember, "not a member of this room"))?;
        if !p.msg_bucket.try_acquire(now.mono_ms) {
            return Err(Error::new(
                ErrorCode::RateLimited,
                "too many messages, slow down",
            ));
        }
        self.last_activity = now.mono_ms;

        let mut out = Outcome::default();
        match msg {
            ClientMessage::Ping(ping) => {
                if let Some(rtt) = ping.rtt_ms {
                    p.tracker.record_rtt(rtt);
                }
                out.only(
                    pid,
                    ServerMessage::Pong {
                        client_time: ping.client_time,
                        server_time: now.wall_ms,
                    },
                );
            }
            ClientMessage::SelectMedia(sel) => {
                self.require_host(pid, "select media")?;
                let media = sel.media.validate()?;
                self.playback.reset_for_media(media.duration_secs, now);
                self.started = false;
                self.last_heartbeat = now.mono_ms;
                info!(room_id = %self.id, by = %pid, media_type = ?media.media_type, provider = ?media.provider, "media selected");
                self.media = Some(media.clone());
                out.all(ServerMessage::MediaSelected {
                    media,
                    playback: self.playback.snapshot(now),
                    by: pid.to_owned(),
                });
            }
            ClientMessage::PlaybackPlay(p) => {
                self.require_playback(pid, p.sequence)?;
                let position = self.check_position(p.position)?;
                if self.playback.play(position, now) {
                    self.started = true;
                    self.last_heartbeat = now.mono_ms;
                    out.all(ServerMessage::PlaybackPlay(self.event(pid, now)));
                } else {
                    out.only(pid, self.sync_state(SyncReason::Stale, now));
                }
            }
            ClientMessage::PlaybackPause(p) => {
                self.require_playback(pid, p.sequence)?;
                let position = self.check_position(p.position)?;
                if self.playback.pause(position, now) {
                    out.all(ServerMessage::PlaybackPause(self.event(pid, now)));
                } else {
                    out.only(pid, self.sync_state(SyncReason::Stale, now));
                }
            }
            ClientMessage::PlaybackSeek(p) => {
                self.require_playback(pid, p.sequence)?;
                let position = self.check_position(p.position)?.ok_or_else(|| {
                    Error::new(ErrorCode::InvalidPosition, "seek requires a position")
                })?;
                self.playback.seek(position, now);
                self.last_heartbeat = now.mono_ms;
                out.all(ServerMessage::PlaybackSeek(self.event(pid, now)));
            }
            ClientMessage::PlaybackRateChanged(r) => {
                self.require_playback(pid, r.sequence)?;
                if !r.rate.is_finite() || r.rate < self.cfg.rate_min || r.rate > self.cfg.rate_max {
                    return Err(Error::new(
                        ErrorCode::InvalidRate,
                        format!(
                            "rate must be between {} and {}",
                            self.cfg.rate_min, self.cfg.rate_max
                        ),
                    ));
                }
                if self.playback.set_rate(r.rate, now) {
                    out.all(ServerMessage::PlaybackRateChanged(self.event(pid, now)));
                } else {
                    out.only(pid, self.sync_state(SyncReason::Stale, now));
                }
            }
            ClientMessage::SyncRequest => {
                out.only(pid, self.sync_state(SyncReason::Requested, now));
            }
            ClientMessage::SyncReport(report) => self.on_sync_report(pid, report, now, &mut out)?,
            ClientMessage::ChatMessage(c) => {
                if !self.chat_enabled {
                    return Err(Error::new(
                        ErrorCode::ChatDisabled,
                        "chat is disabled in this room",
                    ));
                }
                // Re-borrow: `p` was moved into earlier arms' scope, look it up again.
                let part = self
                    .participants
                    .get_mut(pid)
                    .ok_or_else(|| Error::new(ErrorCode::NotMember, "not a member of this room"))?;
                if !part.info.can_chat {
                    return Err(Error::new(
                        ErrorCode::Forbidden,
                        "your token does not allow chat",
                    ));
                }
                if !part.chat_bucket.try_acquire(now.mono_ms) {
                    return Err(Error::new(
                        ErrorCode::RateLimited,
                        "you are sending messages too fast",
                    ));
                }
                let name = part.info.display_name.clone();
                let message =
                    self.chat
                        .push(&self.cfg.chat, &self.id, pid, &name, &c.text, now.wall_ms)?;
                out.all(ServerMessage::ChatMessage(message));
            }
            ClientMessage::UpdateRoom(u) => {
                self.require_host(pid, "change room settings")?;
                self.update_settings(u, now, &mut out);
            }
            ClientMessage::CloseRoom => {
                self.require_host(pid, "close the room")?;
                self.close(ClosedReason::HostClosed, &mut out);
            }
            ClientMessage::LeaveRoom => {
                self.remove_participant(pid, LeaveReason::Left, now, &mut out);
            }
        }
        Ok(out)
    }

    fn update_settings(&mut self, u: UpdateRoomPayload, now: Time, out: &mut Outcome) {
        let mut changed = false;
        if let Some(mode) = u.control_mode
            && mode != self.control_mode
        {
            self.control_mode = mode;
            changed = true;
        }
        if let Some(chat) = u.chat_enabled
            && chat != self.chat_enabled
        {
            self.chat_enabled = chat;
            changed = true;
        }
        if changed {
            out.all(self.room_updated(RoomUpdateReason::SettingsChanged, now));
        }
    }

    fn on_sync_report(
        &mut self,
        pid: &str,
        report: SyncReportPayload,
        now: Time,
        out: &mut Outcome,
    ) -> Result<()> {
        let position = self.check_position(Some(report.position))?.unwrap_or(0.0);
        let current_seq = self.playback.sequence();
        if report.sequence.is_some_and(|s| s > current_seq) {
            return Err(Error::new(
                ErrorCode::InvalidSequence,
                "sequence is ahead of the room's sequence",
            ));
        }
        let canonical = Canonical {
            status: self.playback.status(),
            position: self.playback.position_at(now),
            rate: self.playback.rate(),
        };
        let cfg = self.cfg.clone();
        let Some(p) = self.participants.get_mut(pid) else {
            return Ok(());
        };
        if let Some(rtt) = report.rtt_ms {
            p.tracker.record_rtt(rtt);
        }
        if self.media.is_none() || report.buffering {
            return Ok(());
        }
        let stale = report.sequence.is_some_and(|s| s < current_seq)
            || report.state.is_some_and(|s| s != canonical.status);
        if stale {
            out.only(pid, self.sync_state(SyncReason::Stale, now));
            return Ok(());
        }
        let eval = p.tracker.on_report(&cfg.drift, now, canonical, position);
        out.drift_ms = Some(eval.drift_ms.abs());
        let Some(correction) = eval.correction else {
            return Ok(());
        };
        debug!(room_id = %self.id, participant_id = %pid, drift_ms = eval.drift_ms, ?correction, "sync correction");
        out.corrections += 1;
        if matches!(correction, Correction::Seek { .. }) {
            out.seeks += 1;
        }
        let payload = match correction {
            Correction::AdjustRate { rate, duration_ms } => SyncCorrectionPayload {
                action: CorrectionAction::AdjustRate,
                rate: Some(rate),
                duration_ms: Some(duration_ms),
                position: None,
                drift_ms: eval.drift_ms,
                sequence: current_seq,
                server_time: now.wall_ms,
            },
            Correction::Seek { position } => SyncCorrectionPayload {
                action: CorrectionAction::Seek,
                rate: None,
                duration_ms: None,
                position: Some(position),
                drift_ms: eval.drift_ms,
                sequence: current_seq,
                server_time: now.wall_ms,
            },
        };
        out.only(pid, ServerMessage::SyncCorrection(payload));
        Ok(())
    }

    // ------------------------------------------------------------ validation helpers

    fn event(&self, by: &str, now: Time) -> PlaybackEvent {
        PlaybackEvent {
            by: by.to_owned(),
            playback: self.playback.snapshot(now),
        }
    }

    fn require_host(&self, pid: &str, what: &str) -> Result<()> {
        if pid == self.host {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::NotHost,
                format!("Only the room host can {what}."),
            ))
        }
    }

    /// Permission, media presence and sequence sanity for a playback command.
    fn require_playback(&self, pid: &str, sequence: Option<u64>) -> Result<()> {
        if pid != self.host && self.control_mode == ControlMode::HostOnly {
            return Err(Error::new(
                ErrorCode::ControlDenied,
                "Only the host can control playback in this room.",
            ));
        }
        if self.media.is_none() {
            return Err(Error::new(ErrorCode::NoMedia, "no media has been selected"));
        }
        if sequence.is_some_and(|s| s > self.playback.sequence()) {
            return Err(Error::new(
                ErrorCode::InvalidSequence,
                "sequence is ahead of the room's sequence",
            ));
        }
        Ok(())
    }

    fn check_position(&self, position: Option<f64>) -> Result<Option<f64>> {
        match position {
            None => Ok(None),
            Some(p) if p.is_finite() && p >= 0.0 && p <= self.cfg.max_position_secs => Ok(Some(p)),
            Some(_) => Err(Error::new(
                ErrorCode::InvalidPosition,
                format!(
                    "position must be between 0 and {} seconds",
                    self.cfg.max_position_secs
                ),
            )),
        }
    }
}

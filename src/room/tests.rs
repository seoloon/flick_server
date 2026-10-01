//! Deterministic unit tests of the room state machine (no sockets, no real time).

use std::sync::Arc;

use super::*;
use crate::errors::ErrorCode;
use crate::protocol::{
    ChatSendPayload, ClientMessage, ClosedReason, ControlMode, CorrectionAction, MediaRef,
    MediaType, PingPayload, PositionPayload, Presence, Provider, RatePayload, RoomLifecycle,
    RoomUpdateReason, SelectMediaPayload, ServerMessage, SyncReason, SyncReportPayload,
    UpdateRoomPayload,
};
use crate::sync::clock::Time;
use crate::sync::playback::PlaybackStatus;

fn t(ms: u64) -> Time {
    Time {
        mono_ms: ms,
        wall_ms: 1_700_000_000_000 + ms,
    }
}

fn info(id: &str) -> ParticipantInfo {
    ParticipantInfo {
        id: id.into(),
        display_name: id.to_uppercase(),
        can_chat: true,
    }
}

fn media() -> MediaRef {
    MediaRef {
        provider: Provider::Jellyfin,
        server_id: "jf1".into(),
        media_id: "abc".into(),
        media_type: MediaType::Movie,
        season_id: None,
        episode_id: None,
        title: None,
        duration_secs: None,
    }
}

/// Messages a given participant would receive from an outcome.
fn to(out: &Outcome, pid: &str) -> Vec<ServerMessage> {
    out.deliveries
        .iter()
        .filter(|d| match &d.target {
            Target::All => true,
            Target::AllExcept(x) => x != pid,
            Target::Only(x) => x == pid,
        })
        .map(|d| d.message.clone())
        .collect()
}

fn cfg() -> RoomConfig {
    RoomConfig {
        // Keep rate limiting out of the way unless a test is about it.
        msg_burst: 10_000.0,
        msg_rate_per_sec: 10_000.0,
        ..RoomConfig::default()
    }
}

fn new_room(cfg: RoomConfig) -> Room {
    Room::new(
        "ROOM".into(),
        "flick-1".into(),
        info("host"),
        cfg.default_control_mode,
        cfg.chat_enabled,
        Arc::new(cfg),
        t(0),
    )
}

/// A room whose host and `guests` are all connected.
fn room_with(cfg: RoomConfig, guests: &[&str]) -> Room {
    let mut r = new_room(cfg);
    r.connect("host", t(0)).unwrap();
    for g in guests {
        r.join(info(g), t(0)).unwrap();
        r.connect(g, t(0)).unwrap();
    }
    r
}

fn pos(p: f64) -> PositionPayload {
    PositionPayload {
        position: Some(p),
        sequence: None,
    }
}

fn none() -> PositionPayload {
    PositionPayload::default()
}

fn select(r: &mut Room, now: u64) {
    r.handle(
        "host",
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
        t(now),
    )
    .unwrap();
}

fn err(r: &mut Room, pid: &str, m: ClientMessage, now: u64) -> ErrorCode {
    r.handle(pid, m, t(now)).unwrap_err().code
}

fn report(position: f64) -> SyncReportPayload {
    SyncReportPayload {
        position,
        sequence: None,
        state: None,
        buffering: false,
        rtt_ms: None,
    }
}

// ---------------------------------------------------------------- lifecycle

#[test]
fn lifecycle_follows_media_and_playback() {
    let mut r = room_with(cfg(), &[]);
    assert_eq!(r.lifecycle(), RoomLifecycle::Waiting);
    select(&mut r, 10);
    assert_eq!(r.lifecycle(), RoomLifecycle::MediaSelected);
    r.handle("host", ClientMessage::PlaybackPlay(none()), t(20))
        .unwrap();
    assert_eq!(r.lifecycle(), RoomLifecycle::Playing);
    r.handle("host", ClientMessage::PlaybackPause(none()), t(30))
        .unwrap();
    assert_eq!(r.lifecycle(), RoomLifecycle::Paused);
    // New media goes back to MediaSelected.
    select(&mut r, 40);
    assert_eq!(r.lifecycle(), RoomLifecycle::MediaSelected);
}

#[test]
fn creating_a_room_registers_the_host_and_starts_waiting() {
    let r = new_room(cfg());
    assert_eq!(r.host_id(), "host");
    assert!(r.has_participant("host"));
    assert_eq!(r.presence_of("host"), Some(Presence::Disconnected));
    let v = r.view(t(0));
    assert_eq!(v.state, RoomLifecycle::Waiting);
    assert!(v.participants[0].is_host);
}

#[test]
fn joiner_receives_complete_state_and_others_are_notified() {
    let mut r = room_with(cfg(), &[]);
    select(&mut r, 0);
    r.handle("host", ClientMessage::PlaybackPlay(pos(100.0)), t(0))
        .unwrap();

    let joined = r.join(info("bob"), t(2_000)).unwrap();
    assert!(to(&joined, "host")
        .iter()
        .any(|m| matches!(m, ServerMessage::ParticipantJoined { participant } if participant.participant_id == "bob")));
    assert!(
        to(&joined, "bob").is_empty(),
        "joiner does not get its own join event"
    );

    let connected = r.connect("bob", t(2_000)).unwrap();
    let state = to(&connected, "bob")
        .into_iter()
        .find_map(|m| match m {
            ServerMessage::RoomState {
                room,
                you,
                server_time,
            } => Some((room, you, server_time)),
            _ => None,
        })
        .expect("room_state");
    let (room, you, server_time) = state;
    assert_eq!(you, "bob");
    assert_eq!(server_time, t(2_000).wall_ms);
    assert_eq!(room.host_id, "host");
    assert_eq!(room.media, Some(media()));
    assert_eq!(room.playback.status, PlaybackStatus::Playing);
    assert!(
        (room.playback.position - 102.0).abs() < 1e-9,
        "position advanced 2s"
    );
    assert_eq!(room.playback.rate, 1.0);
    assert_eq!(room.participants.len(), 2);
    assert!(to(&connected, "host").iter().any(|m| matches!(
        m,
        ServerMessage::PresenceChanged {
            presence: Presence::Connected,
            ..
        }
    )));
}

#[test]
fn join_is_idempotent() {
    let mut r = room_with(cfg(), &["bob"]);
    let out = r.join(info("bob"), t(5)).unwrap();
    assert!(out.deliveries.is_empty());
    assert_eq!(r.participant_count(), 2);
}

#[test]
fn full_room_rejects_new_members_but_not_returning_ones() {
    let mut r = room_with(
        RoomConfig {
            max_participants: 2,
            ..cfg()
        },
        &["bob"],
    );
    assert_eq!(
        r.join(info("carol"), t(0)).unwrap_err().code,
        ErrorCode::RoomFull
    );
    assert!(r.join(info("bob"), t(0)).is_ok());
}

// ---------------------------------------------------------------- permissions

#[test]
fn only_host_selects_media() {
    let mut r = room_with(cfg(), &["bob"]);
    let e = err(
        &mut r,
        "bob",
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
        1,
    );
    assert_eq!(e, ErrorCode::NotHost);
    assert_eq!(r.lifecycle(), RoomLifecycle::Waiting);
}

#[test]
fn invalid_media_is_rejected() {
    let mut r = room_with(cfg(), &[]);
    let mut bad = media();
    bad.media_id = "http://evil.example/x".into();
    let e = err(
        &mut r,
        "host",
        ClientMessage::SelectMedia(SelectMediaPayload { media: bad }),
        1,
    );
    assert_eq!(e, ErrorCode::InvalidMedia);
}

#[test]
fn everyone_mode_lets_guests_control_playback() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    let out = r
        .handle("bob", ClientMessage::PlaybackPlay(none()), t(10))
        .unwrap();
    assert!(matches!(to(&out, "host")[0], ServerMessage::PlaybackPlay(ref e) if e.by == "bob"));
    r.handle("bob", ClientMessage::PlaybackSeek(pos(300.0)), t(20))
        .unwrap();
    r.handle("bob", ClientMessage::PlaybackPause(none()), t(30))
        .unwrap();
}

#[test]
fn host_only_mode_denies_guest_playback_but_allows_host() {
    let mut r = room_with(
        RoomConfig {
            default_control_mode: ControlMode::HostOnly,
            ..cfg()
        },
        &["bob"],
    );
    select(&mut r, 0);
    for m in [
        ClientMessage::PlaybackPlay(none()),
        ClientMessage::PlaybackPause(none()),
        ClientMessage::PlaybackSeek(pos(1.0)),
        ClientMessage::PlaybackRateChanged(RatePayload {
            rate: 1.5,
            sequence: None,
        }),
    ] {
        assert_eq!(err(&mut r, "bob", m, 10), ErrorCode::ControlDenied);
    }
    assert_eq!(r.sequence(), 1, "denied commands changed nothing");
    r.handle("host", ClientMessage::PlaybackPlay(none()), t(20))
        .unwrap();
}

#[test]
fn host_can_change_control_mode_guests_cannot() {
    let mut r = room_with(cfg(), &["bob"]);
    let upd = || {
        ClientMessage::UpdateRoom(UpdateRoomPayload {
            control_mode: Some(ControlMode::HostOnly),
            chat_enabled: None,
        })
    };
    assert_eq!(err(&mut r, "bob", upd(), 1), ErrorCode::NotHost);
    let out = r.handle("host", upd(), t(2)).unwrap();
    assert!(matches!(
        to(&out, "bob")[0],
        ServerMessage::RoomUpdated {
            control_mode: ControlMode::HostOnly,
            reason: RoomUpdateReason::SettingsChanged,
            ..
        }
    ));
    // Applying the same setting again is a no-op.
    assert!(r.handle("host", upd(), t(3)).unwrap().deliveries.is_empty());
    select(&mut r, 4);
    assert_eq!(
        err(&mut r, "bob", ClientMessage::PlaybackPlay(none()), 5),
        ErrorCode::ControlDenied
    );
}

#[test]
fn non_members_and_leavers_cannot_act() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    assert_eq!(
        err(&mut r, "mallory", ClientMessage::PlaybackPlay(none()), 1),
        ErrorCode::NotMember
    );
    assert_eq!(
        err(
            &mut r,
            "mallory",
            ClientMessage::ChatMessage(ChatSendPayload { text: "hi".into() }),
            1
        ),
        ErrorCode::NotMember
    );
    r.leave("bob", t(2)).unwrap();
    assert_eq!(
        err(&mut r, "bob", ClientMessage::PlaybackPlay(none()), 3),
        ErrorCode::NotMember
    );
    assert_eq!(r.leave("bob", t(4)).unwrap_err().code, ErrorCode::NotMember);
}

#[test]
fn only_host_can_close_the_room() {
    let mut r = room_with(cfg(), &["bob"]);
    assert_eq!(
        err(&mut r, "bob", ClientMessage::CloseRoom, 1),
        ErrorCode::NotHost
    );
    let out = r.handle("host", ClientMessage::CloseRoom, t(2)).unwrap();
    assert!(out.closed);
    assert!(matches!(
        to(&out, "bob")[0],
        ServerMessage::RoomClosed {
            reason: ClosedReason::HostClosed
        }
    ));
    assert!(r.is_closed());
    assert_eq!(
        err(&mut r, "host", ClientMessage::PlaybackPlay(none()), 3),
        ErrorCode::RoomClosed
    );
}

// ---------------------------------------------------------------- playback

#[test]
fn playback_commands_require_media() {
    let mut r = room_with(cfg(), &[]);
    for m in [
        ClientMessage::PlaybackPlay(none()),
        ClientMessage::PlaybackPause(none()),
        ClientMessage::PlaybackSeek(pos(1.0)),
        ClientMessage::PlaybackRateChanged(RatePayload {
            rate: 1.0,
            sequence: None,
        }),
    ] {
        assert_eq!(err(&mut r, "host", m, 1), ErrorCode::NoMedia);
    }
}

#[test]
fn invalid_positions_rates_and_sequences_are_rejected() {
    let mut r = room_with(cfg(), &[]);
    select(&mut r, 0);
    for p in [-1.0, f64::NAN, f64::INFINITY, 1e12] {
        assert_eq!(
            err(&mut r, "host", ClientMessage::PlaybackSeek(pos(p)), 1),
            ErrorCode::InvalidPosition,
            "{p}"
        );
        assert_eq!(
            err(&mut r, "host", ClientMessage::PlaybackPlay(pos(p)), 1),
            ErrorCode::InvalidPosition
        );
    }
    assert_eq!(
        err(&mut r, "host", ClientMessage::PlaybackSeek(none()), 1),
        ErrorCode::InvalidPosition,
        "seek needs a position"
    );
    for rate in [0.0, -1.0, 0.1, 10.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            err(
                &mut r,
                "host",
                ClientMessage::PlaybackRateChanged(RatePayload {
                    rate,
                    sequence: None
                }),
                1
            ),
            ErrorCode::InvalidRate,
            "{rate}"
        );
    }
    let ahead = PositionPayload {
        position: Some(1.0),
        sequence: Some(999),
    };
    assert_eq!(
        err(&mut r, "host", ClientMessage::PlaybackSeek(ahead), 1),
        ErrorCode::InvalidSequence
    );
    assert_eq!(r.sequence(), 1, "only the media selection was applied");
}

#[test]
fn stale_sequence_is_accepted_and_ordered_last_writer_wins() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    // Both clients last saw sequence 1.
    let seen1 = |p: f64| PositionPayload {
        position: Some(p),
        sequence: Some(1),
    };
    r.handle("host", ClientMessage::PlaybackSeek(seen1(10.0)), t(1))
        .unwrap();
    r.handle("bob", ClientMessage::PlaybackSeek(seen1(20.0)), t(1))
        .unwrap();
    assert_eq!(r.sequence(), 3);
    assert_eq!(r.view(t(1)).playback.position, 20.0);
}

#[test]
fn concurrent_seeks_get_strictly_increasing_sequence_numbers() {
    let mut r = room_with(cfg(), &["bob", "carol"]);
    select(&mut r, 0);
    let mut seen = Vec::new();
    for (who, p) in [("host", 500.0), ("bob", 700.0), ("carol", 600.0)] {
        let out = r
            .handle(who, ClientMessage::PlaybackSeek(pos(p)), t(10))
            .unwrap();
        match &to(&out, "host")[0] {
            ServerMessage::PlaybackSeek(e) => seen.push((e.playback.sequence, e.playback.position)),
            m => panic!("unexpected {m:?}"),
        }
    }
    assert_eq!(seen, vec![(2, 500.0), (3, 700.0), (4, 600.0)]);
    assert_eq!(
        r.view(t(10)).playback.position,
        600.0,
        "the last ordered command wins for everyone"
    );
}

#[test]
fn redundant_play_and_pause_do_not_bump_sequence() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    r.handle("host", ClientMessage::PlaybackPlay(none()), t(1))
        .unwrap();
    let seq = r.sequence();
    let out = r
        .handle("bob", ClientMessage::PlaybackPlay(none()), t(2))
        .unwrap();
    assert_eq!(r.sequence(), seq);
    assert_eq!(out.deliveries.len(), 1);
    assert!(matches!(
        &out.deliveries[0].message,
        ServerMessage::SyncState(s) if s.reason == SyncReason::Stale && s.playback.sequence == seq
    ));
    assert_eq!(out.deliveries[0].target, Target::Only("bob".into()));
}

#[test]
fn play_pause_seek_rate_propagate_with_canonical_state() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    let out = r
        .handle("host", ClientMessage::PlaybackPlay(pos(100.0)), t(1_000))
        .unwrap();
    assert!(matches!(&to(&out, "bob")[0], ServerMessage::PlaybackPlay(e)
        if e.playback.position == 100.0 && e.playback.status == PlaybackStatus::Playing));

    let out = r
        .handle(
            "host",
            ClientMessage::PlaybackRateChanged(RatePayload {
                rate: 1.5,
                sequence: None,
            }),
            t(3_000),
        )
        .unwrap();
    match &to(&out, "bob")[0] {
        ServerMessage::PlaybackRateChanged(e) => {
            assert_eq!(e.playback.rate, 1.5);
            assert!(
                (e.playback.position - 102.0).abs() < 1e-9,
                "rate change does not move the position"
            );
        }
        m => panic!("{m:?}"),
    }
    // 2s later at 1.5x => +3s
    let v = r.view(t(5_000));
    assert!((v.playback.position - 105.0).abs() < 1e-9);

    let out = r
        .handle("bob", ClientMessage::PlaybackPause(pos(105.2)), t(5_000))
        .unwrap();
    assert!(
        matches!(&to(&out, "host")[0], ServerMessage::PlaybackPause(e)
        if e.playback.position == 105.2 && e.playback.status == PlaybackStatus::Paused)
    );
    assert_eq!(r.view(t(500_000)).playback.position, 105.2);
}

#[test]
fn media_change_resets_playback_for_everyone() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    r.handle("host", ClientMessage::PlaybackPlay(pos(500.0)), t(1))
        .unwrap();
    let mut next = media();
    next.media_id = "next".into();
    let out = r
        .handle(
            "host",
            ClientMessage::SelectMedia(SelectMediaPayload {
                media: next.clone(),
            }),
            t(2),
        )
        .unwrap();
    match &to(&out, "bob")[0] {
        ServerMessage::MediaSelected {
            media,
            playback,
            by,
        } => {
            assert_eq!(*media, next);
            assert_eq!(playback.position, 0.0);
            assert_eq!(playback.status, PlaybackStatus::Paused);
            assert_eq!(by, "host");
        }
        m => panic!("{m:?}"),
    }
}

#[test]
fn reconnect_after_five_seconds_resyncs_to_canonical_position() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    r.handle("host", ClientMessage::PlaybackPlay(pos(100.0)), t(0))
        .unwrap();
    r.disconnect("bob", t(1_000));
    let out = r.connect("bob", t(6_000)).unwrap();
    let room = to(&out, "bob")
        .into_iter()
        .find_map(|m| match m {
            ServerMessage::RoomState { room, .. } => Some(room),
            _ => None,
        })
        .unwrap();
    assert!((room.playback.position - 106.0).abs() < 1e-9);
}

#[test]
fn ping_returns_pong_with_server_time() {
    let mut r = room_with(cfg(), &[]);
    let out = r
        .handle(
            "host",
            ClientMessage::Ping(PingPayload {
                client_time: 42.5,
                rtt_ms: Some(80.0),
            }),
            t(7),
        )
        .unwrap();
    assert!(matches!(
        &to(&out, "host")[0],
        ServerMessage::Pong { client_time, server_time }
            if *client_time == 42.5 && *server_time == t(7).wall_ms
    ));
    assert_eq!(r.rtt_stats().1, 1);
}

// ---------------------------------------------------------------- sync heartbeat & drift

#[test]
fn heartbeat_only_while_playing_and_at_interval() {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    assert!(
        r.tick(t(60_000)).deliveries.is_empty(),
        "paused: no heartbeat"
    );
    r.handle("host", ClientMessage::PlaybackPlay(pos(10.0)), t(100_000))
        .unwrap();
    assert!(
        r.tick(t(105_000)).deliveries.is_empty(),
        "interval not elapsed"
    );
    let out = r.tick(t(110_000));
    assert!(matches!(
        &to(&out, "bob")[0],
        ServerMessage::SyncState(s) if s.reason == SyncReason::Heartbeat && (s.playback.position - 20.0).abs() < 1e-9
    ));
    assert!(r.tick(t(112_000)).deliveries.is_empty());
    assert!(!r.tick(t(120_000)).deliveries.is_empty());
}

#[test]
fn sync_request_returns_a_snapshot_to_the_requester_only() {
    let mut r = room_with(cfg(), &["bob"]);
    let out = r.handle("bob", ClientMessage::SyncRequest, t(5)).unwrap();
    assert_eq!(out.deliveries.len(), 1);
    assert_eq!(out.deliveries[0].target, Target::Only("bob".into()));
}

fn playing_at_100() -> Room {
    let mut r = room_with(cfg(), &["bob"]);
    select(&mut r, 0);
    r.handle("host", ClientMessage::PlaybackPlay(pos(100.0)), t(0))
        .unwrap();
    r
}

#[test]
fn in_sync_report_produces_no_correction() {
    let mut r = playing_at_100();
    let out = r
        .handle("bob", ClientMessage::SyncReport(report(105.04)), t(5_000))
        .unwrap();
    assert!(out.deliveries.is_empty());
    assert_eq!(out.corrections, 0);
}

#[test]
fn drifting_client_gets_graduated_corrections() {
    let mut r = playing_at_100();
    // 300ms behind: gentle rate adjustment.
    let out = r
        .handle("bob", ClientMessage::SyncReport(report(104.7)), t(5_000))
        .unwrap();
    match &to(&out, "bob")[0] {
        ServerMessage::SyncCorrection(c) => {
            assert_eq!(c.action, CorrectionAction::AdjustRate);
            assert!(c.rate.unwrap() > 1.0);
            assert!(c.drift_ms < 0.0, "negative = behind");
        }
        m => panic!("{m:?}"),
    }
    assert_eq!(out.corrections, 1);
    // Far behind: hard seek to the canonical position.
    let out = r
        .handle("bob", ClientMessage::SyncReport(report(90.0)), t(20_000))
        .unwrap();
    match &to(&out, "bob")[0] {
        ServerMessage::SyncCorrection(c) => {
            assert_eq!(c.action, CorrectionAction::Seek);
            assert!((c.position.unwrap() - 120.0).abs() < 1e-9);
        }
        m => panic!("{m:?}"),
    }
    // The correction goes to the drifting client only.
    assert!(to(&out, "host").is_empty());
}

#[test]
fn rtt_compensation_is_applied_to_reports() {
    let mut r = playing_at_100();
    let mut rep = report(104.85);
    rep.rtt_ms = Some(300.0);
    // 150ms behind as measured, but the report spent 150ms in flight: in sync.
    let out = r
        .handle("bob", ClientMessage::SyncReport(rep), t(5_000))
        .unwrap();
    assert!(out.deliveries.is_empty());
}

#[test]
fn stale_or_mismatching_reports_get_a_snapshot_not_a_correction() {
    let mut r = playing_at_100();
    let mut stale = report(5.0);
    stale.sequence = Some(0);
    let out = r
        .handle("bob", ClientMessage::SyncReport(stale), t(1_000))
        .unwrap();
    assert!(
        matches!(&to(&out, "bob")[0], ServerMessage::SyncState(s) if s.reason == SyncReason::Stale)
    );
    let mut paused = report(101.0);
    paused.state = Some(PlaybackStatus::Paused);
    let out = r
        .handle("bob", ClientMessage::SyncReport(paused), t(1_000))
        .unwrap();
    assert!(matches!(&to(&out, "bob")[0], ServerMessage::SyncState(_)));
}

#[test]
fn buffering_clients_and_missing_media_are_not_corrected() {
    let mut r = playing_at_100();
    let mut buf = report(0.0);
    buf.buffering = true;
    assert!(
        r.handle("bob", ClientMessage::SyncReport(buf), t(5_000))
            .unwrap()
            .deliveries
            .is_empty()
    );
    let mut empty = room_with(cfg(), &[]);
    assert!(
        empty
            .handle("host", ClientMessage::SyncReport(report(50.0)), t(1))
            .unwrap()
            .deliveries
            .is_empty()
    );
}

#[test]
fn report_with_invalid_position_or_future_sequence_is_rejected() {
    let mut r = playing_at_100();
    assert_eq!(
        err(&mut r, "bob", ClientMessage::SyncReport(report(-5.0)), 1),
        ErrorCode::InvalidPosition
    );
    let mut fut = report(1.0);
    fut.sequence = Some(99);
    assert_eq!(
        err(&mut r, "bob", ClientMessage::SyncReport(fut), 1),
        ErrorCode::InvalidSequence
    );
}

// ---------------------------------------------------------------- host policy, presence, expiry

#[test]
fn host_leaving_transfers_ownership_to_the_longest_standing_connected_member() {
    let mut r = room_with(cfg(), &["bob", "carol"]);
    let out = r.leave("host", t(10)).unwrap();
    assert_eq!(r.host_id(), "bob");
    assert!(!out.closed);
    assert!(out.removed.contains(&"host".to_string()));
    let msgs = to(&out, "carol");
    assert!(msgs.iter().any(|m| matches!(m, ServerMessage::ParticipantLeft { participant_id, .. } if participant_id == "host")));
    assert!(msgs.iter().any(|m| matches!(m, ServerMessage::RoomUpdated { host_id, reason: RoomUpdateReason::HostChanged, .. } if host_id == "bob")));
    // The new host can now select media; the old host cannot.
    r.handle(
        "bob",
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
        t(11),
    )
    .unwrap();
}

#[test]
fn host_transfer_prefers_connected_participants() {
    let mut r = room_with(cfg(), &["bob", "carol"]);
    r.disconnect("bob", t(5));
    r.leave("host", t(10)).unwrap();
    assert_eq!(r.host_id(), "carol");
}

#[test]
fn close_policy_closes_the_room_when_the_host_leaves() {
    let mut r = room_with(
        RoomConfig {
            host_leave_policy: HostLeavePolicy::Close,
            ..cfg()
        },
        &["bob"],
    );
    let out = r.leave("host", t(10)).unwrap();
    assert!(out.closed);
    assert!(to(&out, "bob").iter().any(|m| matches!(
        m,
        ServerMessage::RoomClosed {
            reason: ClosedReason::HostLeft
        }
    )));
}

#[test]
fn dropped_connection_keeps_the_seat_during_grace_and_can_rejoin() {
    let mut r = room_with(cfg(), &["bob"]);
    let out = r.disconnect("bob", t(1_000));
    assert!(matches!(
        &to(&out, "host")[0],
        ServerMessage::PresenceChanged {
            presence: Presence::Reconnecting,
            ..
        }
    ));
    assert_eq!(r.presence_of("bob"), Some(Presence::Reconnecting));
    // Within the 30s grace period nothing happens.
    assert!(r.tick(t(25_000)).deliveries.is_empty());
    assert!(r.has_participant("bob"));
    // Reconnect restores the same identity.
    let out = r.connect("bob", t(26_000)).unwrap();
    assert!(to(&out, "host").iter().any(|m| matches!(
        m,
        ServerMessage::PresenceChanged {
            presence: Presence::Connected,
            ..
        }
    )));
    assert!(r.tick(t(200_000)).removed.is_empty());
    assert_eq!(r.presence_of("bob"), Some(Presence::Connected));
}

#[test]
fn grace_expiry_removes_the_participant() {
    let mut r = room_with(cfg(), &["bob"]);
    r.disconnect("bob", t(1_000));
    let out = r.tick(t(31_000));
    assert_eq!(out.removed, vec!["bob".to_string()]);
    assert!(to(&out, "host").iter().any(|m| matches!(
        m,
        ServerMessage::ParticipantLeft {
            reason: crate::protocol::LeaveReason::Timeout,
            ..
        }
    )));
    assert!(!r.has_participant("bob"));
}

#[test]
fn host_keeps_ownership_while_reconnecting_but_not_after_timeout() {
    let mut r = room_with(cfg(), &["bob"]);
    r.disconnect("host", t(1_000));
    r.tick(t(10_000));
    assert_eq!(r.host_id(), "host");
    r.tick(t(31_000));
    assert_eq!(r.host_id(), "bob");
}

#[test]
fn never_connected_join_expires_after_connect_grace() {
    let mut r = new_room(cfg());
    r.join(info("bob"), t(0)).unwrap();
    assert!(r.tick(t(59_000)).removed.is_empty());
    let out = r.tick(t(60_000));
    assert_eq!(out.removed.len(), 2, "host and bob never connected");
}

#[test]
fn empty_room_is_destroyed_after_the_timeout_and_can_be_revived_before() {
    let mut r = room_with(cfg(), &[]);
    r.leave("host", t(1_000)).unwrap();
    assert_eq!(r.lifecycle(), RoomLifecycle::Empty);
    assert!(!r.tick(t(30_000)).closed);

    // Somebody joins in time: the room lives on.
    r.join(info("bob"), t(40_000)).unwrap();
    assert_ne!(r.lifecycle(), RoomLifecycle::Empty);
    assert!(!r.tick(t(45_000)).closed);
    assert_eq!(r.participant_count(), 1);
}

#[test]
fn empty_room_closes_once_the_timeout_elapses() {
    let mut r = room_with(
        RoomConfig {
            empty_timeout_ms: 5_000,
            ..cfg()
        },
        &[],
    );
    r.leave("host", t(1_000)).unwrap();
    assert!(!r.tick(t(5_999)).closed);
    let out = r.tick(t(6_000));
    assert!(out.closed);
    assert!(r.is_closed());
    assert_eq!(r.lifecycle(), RoomLifecycle::Closed);
}

#[test]
fn idle_room_expires() {
    let mut r = room_with(
        RoomConfig {
            idle_timeout_ms: 10_000,
            ..cfg()
        },
        &["bob"],
    );
    r.handle("bob", ClientMessage::SyncRequest, t(5_000))
        .unwrap();
    assert!(!r.tick(t(14_000)).closed, "activity resets the idle timer");
    let out = r.tick(t(15_000));
    assert!(out.closed);
    assert!(to(&out, "bob").iter().any(|m| matches!(
        m,
        ServerMessage::RoomClosed {
            reason: ClosedReason::Expired
        }
    )));
}

#[test]
fn shutdown_closes_the_room() {
    let mut r = room_with(cfg(), &["bob"]);
    let out = r.shutdown();
    assert!(out.closed);
    assert!(matches!(
        to(&out, "bob")[0],
        ServerMessage::RoomClosed {
            reason: ClosedReason::Shutdown
        }
    ));
}

// ---------------------------------------------------------------- chat & flood control

fn say(r: &mut Room, who: &str, text: &str, now: u64) -> crate::errors::Result<Outcome> {
    r.handle(
        who,
        ClientMessage::ChatMessage(ChatSendPayload { text: text.into() }),
        t(now),
    )
}

#[test]
fn chat_is_broadcast_with_sender_identity_and_timestamp() {
    let mut r = room_with(cfg(), &["bob"]);
    let out = say(&mut r, "bob", "  hello \u{0007}there ", 1_234).unwrap();
    match &to(&out, "host")[0] {
        ServerMessage::ChatMessage(m) => {
            assert_eq!(m.text, "hello there");
            assert_eq!(m.sender_id, "bob");
            assert_eq!(m.sender_name, "BOB");
            assert_eq!(m.room_id, "ROOM");
            assert_eq!(m.timestamp, t(1_234).wall_ms);
            assert_eq!(m.id, 1);
        }
        m => panic!("{m:?}"),
    }
}

#[test]
fn chat_history_is_sent_on_connect_and_bounded() {
    let mut r = room_with(
        RoomConfig {
            chat: crate::chat::ChatConfig {
                max_history: 2,
                burst: 100.0,
                ..Default::default()
            },
            ..cfg()
        },
        &[],
    );
    for i in 0..5 {
        say(&mut r, "host", &format!("m{i}"), i).unwrap();
    }
    r.join(info("bob"), t(10)).unwrap();
    let out = r.connect("bob", t(10)).unwrap();
    let hist = to(&out, "bob")
        .into_iter()
        .find_map(|m| match m {
            ServerMessage::ChatHistory { messages } => Some(messages),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        hist.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["m3", "m4"]
    );
}

#[test]
fn chat_validation_and_permissions() {
    let mut r = room_with(cfg(), &["bob"]);
    assert_eq!(
        say(&mut r, "bob", "   ", 1).unwrap_err().code,
        ErrorCode::InvalidPayload
    );
    assert_eq!(
        say(&mut r, "bob", &"x".repeat(501), 1).unwrap_err().code,
        ErrorCode::MessageTooLarge
    );

    let mut mute = new_room(cfg());
    mute.join(
        ParticipantInfo {
            can_chat: false,
            ..info("quiet")
        },
        t(0),
    )
    .unwrap();
    assert_eq!(
        say(&mut mute, "quiet", "hi", 1).unwrap_err().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn chat_can_be_disabled_by_the_host() {
    let mut r = room_with(cfg(), &["bob"]);
    r.handle(
        "host",
        ClientMessage::UpdateRoom(UpdateRoomPayload {
            control_mode: None,
            chat_enabled: Some(false),
        }),
        t(1),
    )
    .unwrap();
    assert_eq!(
        say(&mut r, "bob", "hi", 2).unwrap_err().code,
        ErrorCode::ChatDisabled
    );
}

#[test]
fn chat_flood_is_rate_limited_per_participant() {
    let mut r = room_with(cfg(), &["bob", "carol"]);
    let mut accepted = 0;
    for _ in 0..20 {
        match say(&mut r, "bob", "spam", 100) {
            Ok(_) => accepted += 1,
            Err(e) => assert_eq!(e.code, ErrorCode::RateLimited),
        }
    }
    assert_eq!(
        accepted, 5,
        "burst of 5, nothing refilled within the same instant"
    );
    assert!(
        say(&mut r, "carol", "I can still talk", 100).is_ok(),
        "limits are per participant"
    );
    assert!(say(&mut r, "bob", "later", 2_100).is_ok(), "bucket refills");
}

#[test]
fn global_message_rate_limit_applies() {
    let mut r = room_with(
        RoomConfig {
            msg_burst: 3.0,
            msg_rate_per_sec: 1.0,
            ..cfg()
        },
        &[],
    );
    for _ in 0..3 {
        r.handle("host", ClientMessage::SyncRequest, t(0)).unwrap();
    }
    assert_eq!(
        err(&mut r, "host", ClientMessage::SyncRequest, 0),
        ErrorCode::RateLimited
    );
    assert!(
        r.handle("host", ClientMessage::SyncRequest, t(1_000))
            .is_ok()
    );
}

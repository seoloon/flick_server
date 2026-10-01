//! Concurrency tests against the room manager (no sockets): many tasks hammering one room.

use std::collections::HashSet;
use std::sync::Arc;

use flicksync::auth::Identity;
use flicksync::errors::ErrorCode;
use flicksync::metrics::Metrics;
use flicksync::protocol::{
    ClientMessage, MediaRef, MediaType, PositionPayload, Provider, RatePayload, SelectMediaPayload,
};
use flicksync::room::manager::{CreateOptions, ManagerConfig, Outbound};
use flicksync::room::{Attachment, RoomConfig, RoomManager};
use flicksync::sync::SystemClock;
use serde_json::Value;

fn ident(user: &str) -> Identity {
    Identity::new(user, "flick-1", user, ["*".to_string()])
}

fn manager(room: RoomConfig) -> Arc<RoomManager> {
    Arc::new(RoomManager::new(
        ManagerConfig {
            room: Arc::new(room),
            max_rooms: 1000,
            create_per_minute: 10_000.0,
            outbound_buffer: 100_000,
        },
        Arc::new(SystemClock::new()),
        Arc::new(Metrics::default()),
    ))
}

fn lenient() -> RoomConfig {
    RoomConfig {
        msg_burst: 1e9,
        msg_rate_per_sec: 1e9,
        reconnect_grace_ms: 60_000,
        ..RoomConfig::default()
    }
}

fn media() -> MediaRef {
    MediaRef {
        provider: Provider::Plex,
        server_id: "p1".into(),
        media_id: "m1".into(),
        media_type: MediaType::Movie,
        season_id: None,
        episode_id: None,
        title: None,
        duration_secs: None,
    }
}

fn drain(att: &mut Attachment) -> Vec<Value> {
    let mut v = Vec::new();
    while let Ok(m) = att.rx.try_recv() {
        if let Outbound::Text(t) = m {
            v.push(serde_json::from_str(&t).unwrap());
        }
    }
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_joins_never_exceed_room_capacity() {
    let m = manager(RoomConfig {
        max_participants: 20,
        ..lenient()
    });
    let created = m
        .create_room(&ident("host"), CreateOptions::default())
        .unwrap();
    let room = created.room.room_id.clone();

    let mut tasks = Vec::new();
    for i in 0..100 {
        let (m, room) = (m.clone(), room.clone());
        tasks.push(tokio::spawn(async move {
            m.attach(&room, &ident(&format!("u{i}"))).map(|a| (i, a))
        }));
    }
    let mut ok = 0;
    let mut full = 0;
    let mut keep = Vec::new();
    for t in tasks {
        match t.await.unwrap() {
            Ok(pair) => {
                ok += 1;
                keep.push(pair);
            }
            Err(e) => {
                assert_eq!(e.code, ErrorCode::RoomFull);
                full += 1;
            }
        }
    }
    // The host already holds one seat.
    assert_eq!(ok, 19);
    assert_eq!(full, 81);
    let view = m.get_room(&room, &ident("host")).unwrap();
    assert_eq!(view.participants.len(), 20);
    let ids: HashSet<_> = view
        .participants
        .iter()
        .map(|p| p.participant_id.clone())
        .collect();
    assert_eq!(ids.len(), 20, "no duplicates");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_seeks_are_totally_ordered_and_all_clients_converge() {
    const CLIENTS: usize = 8;
    const SEEKS: usize = 50;
    let m = manager(lenient());
    let room = m
        .create_room(&ident("host"), CreateOptions::default())
        .unwrap()
        .room
        .room_id;

    let mut atts = Vec::new();
    for i in 0..CLIENTS {
        let user = if i == 0 {
            "host".to_string()
        } else {
            format!("u{i}")
        };
        atts.push((user.clone(), m.attach(&room, &ident(&user)).unwrap()));
    }
    let host_conn = atts[0].1.conn_id;
    m.handle_message(
        &room,
        "host",
        host_conn,
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
    )
    .unwrap();

    let mut tasks = Vec::new();
    for (user, att) in &atts {
        let (m, room, user, conn) = (m.clone(), room.clone(), user.clone(), att.conn_id);
        tasks.push(tokio::spawn(async move {
            for k in 0..SEEKS {
                let pos = (k * 10 + 1) as f64;
                m.handle_message(
                    &room,
                    &user,
                    conn,
                    ClientMessage::PlaybackSeek(PositionPayload {
                        position: Some(pos),
                        sequence: None,
                    }),
                )
                .unwrap();
                tokio::task::yield_now().await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // Every client must have seen exactly the same ordered stream of seeks.
    let mut streams: Vec<Vec<(u64, f64)>> = Vec::new();
    for (_, att) in atts.iter_mut() {
        let seeks: Vec<(u64, f64)> = drain(att)
            .into_iter()
            .filter(|v| v["type"] == "playback_seek")
            .map(|v| {
                (
                    v["payload"]["sequence"].as_u64().unwrap(),
                    v["payload"]["position"].as_f64().unwrap(),
                )
            })
            .collect();
        streams.push(seeks);
    }
    let first = &streams[0];
    assert_eq!(first.len(), CLIENTS * SEEKS);
    for w in first.windows(2) {
        assert_eq!(
            w[1].0,
            w[0].0 + 1,
            "sequence numbers are strictly increasing without gaps"
        );
    }
    for s in &streams {
        assert_eq!(s, first, "every participant observes the same order");
    }
    let last = first.last().unwrap();
    let view = m.get_room(&room, &ident("host")).unwrap();
    assert_eq!(view.playback.sequence, last.0);
    assert_eq!(
        view.playback.position, last.1,
        "canonical state equals the last ordered command"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_play_pause_and_rate_changes_stay_consistent() {
    let m = manager(lenient());
    let room = m
        .create_room(&ident("host"), CreateOptions::default())
        .unwrap()
        .room
        .room_id;
    let mut atts = Vec::new();
    for i in 0..6 {
        let user = if i == 0 {
            "host".to_string()
        } else {
            format!("u{i}")
        };
        atts.push((user.clone(), m.attach(&room, &ident(&user)).unwrap()));
    }
    let hc = atts[0].1.conn_id;
    m.handle_message(
        &room,
        "host",
        hc,
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
    )
    .unwrap();

    let mut tasks = Vec::new();
    for (n, (user, att)) in atts.iter().enumerate() {
        let (m, room, user, conn) = (m.clone(), room.clone(), user.clone(), att.conn_id);
        tasks.push(tokio::spawn(async move {
            for k in 0..60 {
                let msg = match (n + k) % 3 {
                    0 => ClientMessage::PlaybackPlay(PositionPayload::default()),
                    1 => ClientMessage::PlaybackPause(PositionPayload::default()),
                    _ => ClientMessage::PlaybackRateChanged(RatePayload {
                        rate: if k % 2 == 0 { 1.0 } else { 1.25 },
                        sequence: None,
                    }),
                };
                m.handle_message(&room, &user, conn, msg).unwrap();
                tokio::task::yield_now().await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // Reconstruct the final state from the broadcast stream of any client and compare with the room.
    let events: Vec<Value> = drain(&mut atts[1].1)
        .into_iter()
        .filter(|v| {
            matches!(
                v["type"].as_str(),
                Some("playback_play" | "playback_pause" | "playback_rate_changed")
            )
        })
        .collect();
    assert!(!events.is_empty());
    let mut prev = 1u64;
    for e in &events {
        let seq = e["payload"]["sequence"].as_u64().unwrap();
        assert!(
            seq > prev,
            "sequence must strictly increase ({prev} -> {seq})"
        );
        prev = seq;
    }
    let last = events.last().unwrap();
    let view = m.get_room(&room, &ident("host")).unwrap();
    assert_eq!(
        view.playback.sequence,
        last["payload"]["sequence"].as_u64().unwrap()
    );
    assert_eq!(
        view.playback.rate,
        last["payload"]["rate"].as_f64().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn join_leave_disconnect_reconnect_churn_keeps_invariants() {
    let m = manager(lenient());
    let room = m
        .create_room(&ident("host"), CreateOptions::default())
        .unwrap()
        .room
        .room_id;
    let host = m.attach(&room, &ident("host")).unwrap();

    let mut tasks = Vec::new();
    for i in 0..16 {
        let (m, room) = (m.clone(), room.clone());
        tasks.push(tokio::spawn(async move {
            let id = ident(&format!("u{i}"));
            for round in 0..40 {
                let Ok(att) = m.attach(&room, &id) else {
                    continue;
                };
                // Several of these race with the same user from other rounds/tasks.
                let _ =
                    m.handle_message(&room, &id.user_id, att.conn_id, ClientMessage::SyncRequest);
                match round % 4 {
                    0 => m.detach(&room, &id.user_id, att.conn_id),
                    1 => {
                        let _ = m.leave(&room, &id);
                    }
                    2 => {
                        // Reconnect storm: a second connection replaces the first.
                        let again = m.attach(&room, &id);
                        m.detach(&room, &id.user_id, att.conn_id);
                        if let Ok(again) = again {
                            m.detach(&room, &id.user_id, again.conn_id);
                        }
                    }
                    _ => {
                        let _ = m.join(&room, &id);
                        let _ = m.leave(&room, &id);
                    }
                }
                tokio::task::yield_now().await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    m.sweep();

    let view = m.get_room(&room, &ident("host")).unwrap();
    let ids: HashSet<_> = view
        .participants
        .iter()
        .map(|p| p.participant_id.clone())
        .collect();
    assert_eq!(
        ids.len(),
        view.participants.len(),
        "no duplicate participants"
    );
    assert!(ids.contains(&view.host_id), "the host is always a member");
    assert_eq!(view.host_id, "host", "the host never left");
    assert_eq!(view.participants.iter().filter(|p| p.is_host).count(), 1);
    drop(host);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_room_creation_respects_the_global_cap() {
    let m = Arc::new(RoomManager::new(
        ManagerConfig {
            room: Arc::new(lenient()),
            max_rooms: 10,
            create_per_minute: 10_000.0,
            outbound_buffer: 16,
        },
        Arc::new(SystemClock::new()),
        Arc::new(Metrics::default()),
    ));
    let mut tasks = Vec::new();
    for i in 0..50 {
        let m = m.clone();
        tasks.push(tokio::spawn(async move {
            m.create_room(&ident(&format!("u{i}")), CreateOptions::default())
                .is_ok()
        }));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap() {
            ok += 1;
        }
    }
    assert_eq!(ok, 10);
    assert_eq!(m.room_count(), 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_consumers_are_detached_without_blocking_the_room() {
    let m = Arc::new(RoomManager::new(
        ManagerConfig {
            room: Arc::new(lenient()),
            max_rooms: 10,
            create_per_minute: 10_000.0,
            outbound_buffer: 4, // tiny queue, nobody drains it
        },
        Arc::new(SystemClock::new()),
        Arc::new(Metrics::default()),
    ));
    let room = m
        .create_room(&ident("host"), CreateOptions::default())
        .unwrap()
        .room
        .room_id;
    let host = m.attach(&room, &ident("host")).unwrap();
    let _slow = m.attach(&room, &ident("slow")).unwrap();
    let mut host = host;
    drain(&mut host);
    // Flood the room: the host keeps draining, "slow" never does.
    m.handle_message(
        &room,
        "host",
        host.conn_id,
        ClientMessage::SelectMedia(SelectMediaPayload { media: media() }),
    )
    .unwrap();
    for i in 0..50 {
        m.handle_message(
            &room,
            "host",
            host.conn_id,
            ClientMessage::PlaybackSeek(PositionPayload {
                position: Some(i as f64),
                sequence: None,
            }),
        )
        .unwrap();
        drain(&mut host);
    }
    let view = m.get_room(&room, &ident("host")).unwrap();
    let slow = view
        .participants
        .iter()
        .find(|p| p.participant_id == "slow")
        .unwrap();
    assert_eq!(
        slow.presence,
        flicksync::protocol::Presence::Reconnecting,
        "the stuck connection was dropped but the seat is kept for reconnection"
    );
}

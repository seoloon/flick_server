//! Protocol hardening: malformed input must produce typed errors and never hurt the server.

mod common;

use common::*;
use serde_json::json;

async fn two_in_room(s: &TestServer) -> (String, Ws, Ws) {
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    a.expect("room_state").await;
    a.expect("chat_history").await;
    b.expect("room_state").await;
    b.expect("chat_history").await;
    a.expect("participant_joined").await;
    a.expect("presence_changed").await;
    (room, a, b)
}

#[tokio::test]
async fn malformed_json_and_unknown_types_yield_errors_and_keep_the_connection() {
    let s = TestServer::start(&[]).await;
    let (_room, mut a, _b) = two_in_room(&s).await;

    let cases: Vec<(&str, &str)> = vec![
        ("this is not json", "INVALID_MESSAGE"),
        ("{", "INVALID_MESSAGE"),
        ("null", "INVALID_MESSAGE"),
        ("[1,2,3]", "INVALID_MESSAGE"),
        ("\"string\"", "INVALID_MESSAGE"),
        ("{}", "INVALID_MESSAGE"),
        (r#"{"type":123}"#, "INVALID_MESSAGE"),
        (r#"{"type":"does_not_exist","payload":{}}"#, "UNKNOWN_TYPE"),
        (r#"{"type":"room_state","payload":{}}"#, "UNKNOWN_TYPE"),
        (
            r#"{"type":"playback_seek","payload":{"position":"soon"}}"#,
            "INVALID_PAYLOAD",
        ),
        (
            r#"{"type":"playback_seek","payload":"oops"}"#,
            "INVALID_PAYLOAD",
        ),
        (
            r#"{"type":"playback_rate_changed","payload":{}}"#,
            "INVALID_PAYLOAD",
        ),
        (
            r#"{"type":"select_media","payload":{"media":{"provider":"plex"}}}"#,
            "INVALID_PAYLOAD",
        ),
        (
            r#"{"type":"chat_message","payload":{"text":42}}"#,
            "INVALID_PAYLOAD",
        ),
        (
            r#"{"protocol_version":99,"type":"sync_request"}"#,
            "UNSUPPORTED_VERSION",
        ),
    ];
    for (raw, code) in cases {
        a.send_raw(raw).await;
        let e = a.expect("error").await;
        assert_eq!(e["code"], code, "input: {raw}");
        assert!(e["message"].is_string());
    }
    // Still alive and well.
    a.send("sync_request", json!({})).await;
    a.expect("sync_state").await;
    assert!(s.state.metrics.malformed_messages_total.get() >= 15);
}

#[tokio::test]
async fn binary_frames_are_rejected_gracefully() {
    let s = TestServer::start(&[]).await;
    let (_r, mut a, _b) = two_in_room(&s).await;
    a.send_binary(vec![0, 1, 2, 3]).await;
    assert_eq!(a.expect("error").await["code"], "INVALID_MESSAGE");
    a.send("sync_request", json!({})).await;
    a.expect("sync_state").await;
}

#[tokio::test]
async fn invalid_values_are_rejected_without_changing_state() {
    let s = TestServer::start(&[]).await;
    let (_room, mut a, mut b) = two_in_room(&s).await;
    a.send("select_media", json!({ "media": media("m") })).await;
    a.expect("media_selected").await;
    b.expect("media_selected").await;

    let bad = [
        (
            "playback_seek",
            json!({ "position": -5.0 }),
            "INVALID_POSITION",
        ),
        (
            "playback_seek",
            json!({ "position": 1e15 }),
            "INVALID_POSITION",
        ),
        ("playback_seek", json!({}), "INVALID_POSITION"),
        (
            "playback_play",
            json!({ "position": -1.0 }),
            "INVALID_POSITION",
        ),
        (
            "playback_rate_changed",
            json!({ "rate": 0.0 }),
            "INVALID_RATE",
        ),
        (
            "playback_rate_changed",
            json!({ "rate": -2.0 }),
            "INVALID_RATE",
        ),
        (
            "playback_rate_changed",
            json!({ "rate": 99.0 }),
            "INVALID_RATE",
        ),
        (
            "playback_seek",
            json!({ "position": 1.0, "sequence": 9999 }),
            "INVALID_SEQUENCE",
        ),
        (
            "sync_report",
            json!({ "position": -3.0 }),
            "INVALID_POSITION",
        ),
        (
            "sync_report",
            json!({ "position": 1.0, "sequence": 9999 }),
            "INVALID_SEQUENCE",
        ),
        (
            "select_media",
            json!({ "media": { "provider": "plex", "server_id": "s", "media_id": "../../etc/passwd", "media_type": "movie" } }),
            "INVALID_MEDIA",
        ),
        (
            "select_media",
            json!({ "media": { "provider": "jellyfin", "server_id": "http://169.254.169.254/", "media_id": "x", "media_type": "movie" } }),
            "INVALID_MEDIA",
        ),
    ];
    for (ty, payload, code) in bad {
        a.send(ty, payload.clone()).await;
        let e = a.expect("error").await;
        assert_eq!(e["code"], code, "{ty} {payload}");
    }
    // State untouched: a fresh snapshot still has sequence 1 (media selection only).
    b.send("sync_request", json!({})).await;
    let snap = b.expect("sync_state").await;
    assert_eq!(snap["sequence"], 1);
    assert_eq!(snap["position"], 0.0);
    assert_eq!(snap["state"], "paused");
}

#[tokio::test]
async fn playback_before_media_selection_is_rejected() {
    let s = TestServer::start(&[]).await;
    let (_r, mut a, _b) = two_in_room(&s).await;
    for ty in ["playback_play", "playback_pause"] {
        a.send(ty, json!({})).await;
        assert_eq!(a.expect("error").await["code"], "NO_MEDIA");
    }
}

#[tokio::test]
async fn oversized_message_closes_the_connection_but_not_the_server() {
    let s = TestServer::start(&[]).await;
    let (room, mut a, mut b) = two_in_room(&s).await;
    // Limit is 4096 bytes in the test configuration.
    let big = format!(
        r#"{{"type":"chat_message","payload":{{"text":"{}"}}}}"#,
        "x".repeat(20_000)
    );
    a.send_raw(&big).await;
    let _ = a.expect_closed().await;

    // Server is fine; the other participant just sees presence change.
    b.expect_where("presence_changed", |p| p["participant_id"] == "alice")
        .await;
    let mut a2 = s.connect(&room, "alice").await;
    a2.expect("room_state").await;
    let (st, _) = s.http(axum::http::Method::GET, "/health", None, None).await;
    assert_eq!(st, axum::http::StatusCode::OK);
}

#[tokio::test]
async fn message_just_under_the_limit_is_still_processed() {
    let s = TestServer::start(&[("FLICKSYNC_CHAT_MAX_LENGTH", "3000")]).await;
    let (_r, mut a, mut b) = two_in_room(&s).await;
    a.send("chat_message", json!({ "text": "y".repeat(2500) }))
        .await;
    let m = b.expect("chat_message").await;
    assert_eq!(m["text"].as_str().unwrap().len(), 2500);
}

#[tokio::test]
async fn chat_validation_over_the_wire() {
    let s = TestServer::start(&[]).await;
    let (_r, mut a, mut b) = two_in_room(&s).await;
    a.send("chat_message", json!({ "text": "   " })).await;
    assert_eq!(a.expect("error").await["code"], "INVALID_PAYLOAD");
    a.send("chat_message", json!({ "text": "z".repeat(600) }))
        .await;
    assert_eq!(a.expect("error").await["code"], "MESSAGE_TOO_LARGE");
    a.send(
        "chat_message",
        json!({ "text": "<script>alert(1)</script>\u{0007}" }),
    )
    .await;
    let m = b.expect("chat_message").await;
    assert_eq!(
        m["text"], "<script>alert(1)</script>",
        "control chars stripped, content untouched"
    );
}

#[tokio::test]
async fn chat_flood_is_rate_limited_over_the_wire() {
    let s = TestServer::start(&[
        ("FLICKSYNC_CHAT_BURST", "3"),
        ("FLICKSYNC_CHAT_RATE_PER_SEC", "0.1"),
    ])
    .await;
    let (_r, mut a, _b) = two_in_room(&s).await;
    for i in 0..3 {
        a.send("chat_message", json!({ "text": format!("m{i}") }))
            .await;
        a.expect("chat_message").await;
    }
    a.send("chat_message", json!({ "text": "too many" })).await;
    assert_eq!(a.expect("error").await["code"], "RATE_LIMITED");
}

#[tokio::test]
async fn message_flood_leads_to_rate_limit_errors_then_disconnect() {
    let s = TestServer::start(&[
        ("FLICKSYNC_MSG_BURST", "5"),
        ("FLICKSYNC_MSG_RATE_PER_SEC", "1"),
        ("FLICKSYNC_WS_RATE_LIMIT_STRIKES", "5"),
    ])
    .await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    for _ in 0..40 {
        a.send_raw(r#"{"type":"sync_request"}"#).await;
    }
    // Drain: we must see rate-limit errors, then the server hangs up with 1008.
    let mut limited = 0;
    while let Some(v) = a.try_next_json(3000).await {
        if v["type"] == "error" && v["payload"]["code"] == "RATE_LIMITED" {
            limited += 1;
        }
    }
    assert!(limited >= 4, "got {limited} rate limit errors");
    assert!(s.state.metrics.rate_limited_total.get() >= 5);
}

#[tokio::test]
async fn unauthorized_commands_are_rejected_for_members_without_rights() {
    let s = TestServer::start(&[("FLICKSYNC_DEFAULT_CONTROL_MODE", "host_only")]).await;
    let (_room, mut a, mut b) = two_in_room(&s).await;
    a.send("select_media", json!({ "media": media("m") })).await;
    a.expect("media_selected").await;
    b.expect("media_selected").await;

    for (ty, payload, code) in [
        (
            "select_media",
            json!({ "media": media("other") }),
            "NOT_HOST",
        ),
        ("close_room", json!({}), "NOT_HOST"),
        (
            "update_room",
            json!({ "control_mode": "everyone" }),
            "NOT_HOST",
        ),
        ("playback_play", json!({}), "CONTROL_DENIED"),
        (
            "playback_seek",
            json!({ "position": 5.0 }),
            "CONTROL_DENIED",
        ),
    ] {
        b.send(ty, payload).await;
        assert_eq!(b.expect("error").await["code"], code, "{ty}");
    }
    // None of it reached the host.
    a.assert_silent(200).await;
}

#[tokio::test]
async fn garbage_fuzz_never_kills_the_connection_or_the_server() {
    let s = TestServer::start(&[("FLICKSYNC_MSG_BURST", "100000")]).await;
    let (room, mut a, _b) = two_in_room(&s).await;
    // Deterministic pseudo-random garbage.
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet: Vec<char> = "{}[]\":,0123456789.-etypload_ \\/null".chars().collect();
    for _ in 0..300 {
        let len = (next() % 80) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        a.send_raw(&text).await;
    }
    // Everything above was either an error reply or (rarely) valid; the socket must still work.
    a.send("sync_request", json!({ "marker": true })).await;
    a.expect("sync_state").await;
    // And the room is still healthy for new joiners.
    let mut c = s.connect(&room, "carol").await;
    c.expect("room_state").await;
}

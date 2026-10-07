//! End-to-end tests against a real server over real WebSockets.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn health_ready_and_metrics() {
    let s = TestServer::start(&[]).await;
    let (st, body) = s.http(Method::GET, "/health", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    let (st, _) = s.http(Method::GET, "/ready", None, None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = s.http(Method::GET, "/metrics", None, None).await;
    assert_eq!(st, StatusCode::OK);

    let off = TestServer::start(&[("FLICKSYNC_METRICS_ENABLED", "false")]).await;
    let (st, _) = off.http(Method::GET, "/metrics", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn metrics_token_is_enforced_when_configured() {
    let s = TestServer::start(&[("FLICKSYNC_METRICS_TOKEN", "sekret")]).await;
    let (st, _) = s.http(Method::GET, "/metrics", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = s.http(Method::GET, "/metrics", Some("wrong"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = s.http(Method::GET, "/metrics", Some("sekret"), None).await;
    assert_eq!(st, StatusCode::OK);
}

/// The scenario from the brief: create -> A -> B -> select -> play -> pause -> seek ->
/// reconnect -> leave -> destruction.
#[tokio::test]
async fn full_watch_together_flow() {
    let s = TestServer::start(&[]).await;

    // create
    let (st, created) = s
        .http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None)
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let room = created["room_id"].as_str().unwrap().to_string();
    assert_eq!(room.len(), 12);
    assert_eq!(created["host_id"], "alice");
    assert_eq!(created["participant_id"], "alice");
    assert_eq!(created["room"]["state"], "waiting");
    assert_eq!(created["ws_path"], format!("/api/v1/rooms/{room}/ws"));
    assert!(created["share_code"].as_str().unwrap().contains('-'));

    // participant A connects
    let mut a = s.connect(&room, "alice").await;
    let state = a.expect("room_state").await;
    assert_eq!(state["you"], "alice");
    assert_eq!(state["room"]["host_id"], "alice");
    a.expect("chat_history").await;

    // participant B joins over REST, then connects
    let (st, joined) = s.join_http(&room, "bob").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(joined["room"]["participants"].as_array().unwrap().len(), 2);
    a.expect_where("participant_joined", |p| {
        p["participant"]["participant_id"] == "bob"
    })
    .await;
    let mut b = s.connect(&room, "bob").await;
    let bstate = b.expect("room_state").await;
    assert_eq!(bstate["room"]["participants"].as_array().unwrap().len(), 2);
    a.expect_where("presence_changed", |p| {
        p["presence"] == "connected" && p["participant_id"] == "bob"
    })
    .await;

    // host selects media: everybody gets it
    a.send("select_media", json!({ "media": media("movie-1") }))
        .await;
    for c in [&mut a, &mut b] {
        let m = c.expect("media_selected").await;
        assert_eq!(m["media"]["media_id"], "movie-1");
        assert_eq!(m["by"], "alice");
        assert_eq!(m["playback"]["state"], "paused");
        assert_eq!(m["playback"]["position"], 0.0);
    }

    // play (by the guest: everyone-mode)
    b.send("playback_play", json!({ "position": 10.0 })).await;
    let pa = a.expect("playback_play").await;
    let pb = b.expect("playback_play").await;
    assert_eq!(pa["by"], "bob");
    assert_eq!(pa["state"], "playing");
    assert_eq!(pa["sequence"], pb["sequence"]);
    assert_eq!(pa["position"], 10.0);

    // rate
    a.send("playback_rate_changed", json!({ "rate": 1.5 }))
        .await;
    assert_eq!(a.expect("playback_rate_changed").await["rate"], 1.5);
    assert_eq!(b.expect("playback_rate_changed").await["rate"], 1.5);

    // seek
    b.send("playback_seek", json!({ "position": 540.25 })).await;
    let sa = a.expect("playback_seek").await;
    let sb = b.expect("playback_seek").await;
    assert_eq!(sa["position"], 540.25);
    assert_eq!(sa["sequence"], sb["sequence"]);
    assert!(sa["server_time"].as_u64().unwrap() > 1_700_000_000_000);

    // pause
    a.send("playback_pause", json!({ "position": 541.0 })).await;
    let pa = a.expect("playback_pause").await;
    assert_eq!(pa["state"], "paused");
    assert_eq!(pa["position"], 541.0);
    assert_eq!(b.expect("playback_pause").await["position"], 541.0);

    // chat
    b.send("chat_message", json!({ "text": "hello alice" }))
        .await;
    let chat = a.expect("chat_message").await;
    assert_eq!(chat["text"], "hello alice");
    assert_eq!(chat["sender_id"], "bob");
    assert_eq!(chat["sender_name"], "BOB");
    b.expect("chat_message").await;

    // ping / pong
    a.send("ping", json!({ "client_time": 12345.5 })).await;
    let pong = a.expect("pong").await;
    assert_eq!(pong["client_time"], 12345.5);
    assert!(pong["server_time"].as_u64().unwrap() > 0);

    // B's connection drops: A sees "reconnecting", B reconnects and gets the same state
    b.drop_abruptly();
    a.expect_where("presence_changed", |p| p["presence"] == "reconnecting")
        .await;
    let mut b = s.connect(&room, "bob").await;
    let rs = b.expect("room_state").await;
    assert_eq!(rs["room"]["playback"]["position"], 541.0);
    assert_eq!(rs["room"]["media"]["media_id"], "movie-1");
    assert_eq!(rs["room"]["participants"].as_array().unwrap().len(), 2);
    let hist = b.expect("chat_history").await;
    assert_eq!(hist["messages"][0]["text"], "hello alice");
    a.expect_where("presence_changed", |p| p["presence"] == "connected")
        .await;

    // B leaves explicitly
    b.send("leave_room", json!({})).await;
    assert_eq!(b.expect_closed().await, Some(4002));
    a.expect_where("participant_left", |p| p["participant_id"] == "bob")
        .await;

    // A leaves via REST: the room empties and is destroyed after the timeout
    let (st, _) = s
        .http(
            Method::POST,
            &format!("/api/v1/rooms/{room}/leave"),
            Some(&token("alice")),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(a.expect_closed().await, Some(4002));
    let mgr = s.state.manager.clone();
    eventually("room destroyed", || mgr.room_count() == 0).await;
    let (st, _) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{room}"),
            Some(&token("alice")),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(s.state.metrics.rooms_active.get(), 0);
    assert_eq!(s.state.metrics.rooms_destroyed_total.get(), 1);
}

#[tokio::test]
async fn authentication_is_required_and_enforced_everywhere() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;

    // REST
    for (m, p) in [
        (Method::POST, "/api/v1/rooms".to_string()),
        (Method::GET, format!("/api/v1/rooms/{room}")),
        (Method::POST, format!("/api/v1/rooms/{room}/join")),
        (Method::POST, format!("/api/v1/rooms/{room}/leave")),
    ] {
        let m_again = m.clone();
        let (st, body) = s.http(m, &p, None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{p}");
        assert_eq!(body["error"]["code"], "UNAUTHENTICATED");
        let (st, _) = s.http(m_again, &p, Some("garbage"), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    // WebSocket upgrade
    assert!(Ws::connect_with(&s, &room, None, &[]).await.is_err());
    assert!(Ws::connect(&s, &room, "garbage").await.is_err());
    assert!(s.state.metrics.auth_failures_total.get() >= 6);
}

#[tokio::test]
async fn token_bound_to_wrong_server_or_missing_permissions_is_refused() {
    let s = TestServer::start(&[]).await;

    // Signed with server 1's key but claiming to be server 2.
    let forged = token_for("mallory", OTHER_SERVER, "k1", SECRET, &["*"]);
    let (st, _) = s
        .http(Method::POST, "/api/v1/rooms", Some(&forged), None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Valid token without the create permission.
    let weak = token_for("carl", SERVER, "k1", SECRET, &["rooms:join"]);
    let (st, body) = s
        .http(Method::POST, "/api/v1/rooms", Some(&weak), None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "FORBIDDEN");

    // Rooms are scoped to the creator's Flick server: another (valid) server cannot even see it.
    let room = s.create_room("alice").await;
    let foreign = token_for("eve", OTHER_SERVER, "k2", OTHER_SECRET, &["*"]);
    let (st, _) = s
        .http(
            Method::POST,
            &format!("/api/v1/rooms/{room}/join"),
            Some(&foreign),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(Ws::connect(&s, &room, &foreign).await.is_err());
}

#[tokio::test]
async fn non_members_cannot_read_room_state_and_bad_ids_are_not_found() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let (st, _) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{room}"),
            Some(&token("mallory")),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{room}"),
            Some(&token("alice")),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["host_id"], "alice");
    for bad in ["nope", "000000000000", "..%2F..%2Fetc"] {
        let (st, _) = s
            .http(
                Method::GET,
                &format!("/api/v1/rooms/{bad}"),
                Some(&token("alice")),
                None,
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{bad}");
    }
    // Share-code form (lower case, hyphens) resolves to the same room.
    let code = format!("{}-{}-{}", &room[..4], &room[4..8], &room[8..]).to_lowercase();
    let (st, _) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{code}"),
            Some(&token("alice")),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn create_room_rejects_garbage_body_and_honours_options() {
    let s = TestServer::start(&[]).await;
    let (st, body) = s
        .http(
            Method::POST,
            "/api/v1/rooms",
            Some(&token("alice")),
            Some(json!({ "control_mode": "host_only", "chat_enabled": false })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(body["room"]["control_mode"], "host_only");
    assert_eq!(body["room"]["chat_enabled"], false);
    let (st, _) = s
        .http(
            Method::POST,
            "/api/v1/rooms",
            Some(&token("alice")),
            Some(json!({ "control_mode": "anarchy" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn room_creation_is_rate_limited_per_user() {
    let s = TestServer::start(&[("FLICKSYNC_ROOM_CREATE_PER_MINUTE", "3")]).await;
    for _ in 0..3 {
        s.create_room("alice").await;
    }
    let (st, body) = s
        .http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None)
        .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "RATE_LIMITED");
    // Another user is unaffected.
    s.create_room("bob").await;
}

#[tokio::test]
async fn global_room_cap_is_enforced() {
    let s = TestServer::start(&[("FLICKSYNC_MAX_ROOMS", "2")]).await;
    s.create_room("a").await;
    s.create_room("b").await;
    let (st, body) = s
        .http(Method::POST, "/api/v1/rooms", Some(&token("c")), None)
        .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "TOO_MANY_ROOMS");
}

#[tokio::test]
async fn room_size_limit_applies_over_rest_and_websocket() {
    let s = TestServer::start(&[("FLICKSYNC_MAX_ROOM_SIZE", "2")]).await;
    let room = s.create_room("alice").await;
    assert_eq!(s.join_http(&room, "bob").await.0, StatusCode::OK);
    let (st, body) = s.join_http(&room, "carol").await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "ROOM_FULL");
    assert!(Ws::connect(&s, &room, &token("carol")).await.is_err());
    // Existing members can still connect.
    let mut b = s.connect(&room, "bob").await;
    b.expect("room_state").await;
}

#[tokio::test]
async fn unlimited_style_rooms_accept_many_participants() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("host").await;
    let mut clients = Vec::new();
    for i in 0..25 {
        let mut c = s.connect(&room, &format!("user{i}")).await;
        c.expect("room_state").await;
        clients.push(c);
    }
    let (_, v) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{room}"),
            Some(&token("user3")),
            None,
        )
        .await;
    assert_eq!(v["participants"].as_array().unwrap().len(), 26);
}

#[tokio::test]
async fn only_the_host_can_select_media_over_the_socket() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    a.expect("room_state").await;
    b.expect("room_state").await;

    b.send("select_media", json!({ "media": media("x") })).await;
    let e = b.expect("error").await;
    assert_eq!(e["code"], "NOT_HOST");
    assert_eq!(e["message"], "Only the room host can select media.");
    // Nothing leaked to the host.
    a.expect_where("participant_joined", |_| true).await;
}

#[tokio::test]
async fn host_only_rooms_reject_guest_playback_commands() {
    let s = TestServer::start(&[("FLICKSYNC_DEFAULT_CONTROL_MODE", "host_only")]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    a.send("select_media", json!({ "media": media("m") })).await;
    b.expect("media_selected").await;
    b.send("playback_play", json!({})).await;
    assert_eq!(b.expect("error").await["code"], "CONTROL_DENIED");
    // The host re-opens control at runtime.
    a.send("update_room", json!({ "control_mode": "everyone" }))
        .await;
    let upd = b.expect("room_updated").await;
    assert_eq!(upd["control_mode"], "everyone");
    b.send("playback_play", json!({})).await;
    assert_eq!(b.expect("playback_play").await["by"], "bob");
}

#[tokio::test]
async fn host_leaving_transfers_ownership_over_websocket() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    b.expect("room_state").await;
    a.send("leave_room", json!({})).await;
    assert_eq!(a.expect_closed().await, Some(4002));
    let upd = b.expect("room_updated").await;
    assert_eq!(upd["host_id"], "bob");
    assert_eq!(upd["reason"], "host_changed");
    b.send("select_media", json!({ "media": media("m") })).await;
    b.expect("media_selected").await;
}

#[tokio::test]
async fn host_can_close_the_room_for_everyone() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    b.expect("room_state").await;
    b.send("close_room", json!({})).await;
    assert_eq!(b.expect("error").await["code"], "NOT_HOST");
    a.send("close_room", json!({})).await;
    assert_eq!(b.expect("room_closed").await["reason"], "host_closed");
    assert_eq!(b.expect_closed().await, Some(4003));
    assert_eq!(a.expect_closed().await, Some(4003));
    let mgr = s.state.manager.clone();
    eventually("room removed", || mgr.room_count() == 0).await;
}

#[tokio::test]
async fn grace_period_expiry_removes_a_participant_who_never_returns() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let b = s.connect(&room, "bob").await;
    b.drop_abruptly();
    a.expect_where("presence_changed", |p| p["presence"] == "reconnecting")
        .await;
    // Grace is 1s in the test configuration.
    let left = a
        .expect_where("participant_left", |p| p["participant_id"] == "bob")
        .await;
    assert_eq!(left["reason"], "timeout");
}

#[tokio::test]
async fn second_connection_replaces_the_first_without_losing_the_seat() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    let mut a2 = s.connect(&room, "alice").await;
    a2.expect("room_state").await;
    a2.expect("chat_history").await;
    assert_eq!(a.expect_closed().await, Some(4001));
    // The old socket's teardown must not flag the participant as disconnected.
    a2.assert_silent(300).await;
    a2.send("sync_request", json!({})).await;
    a2.expect("sync_state").await;
    let (_, v) = s
        .http(
            Method::GET,
            &format!("/api/v1/rooms/{room}"),
            Some(&token("alice")),
            None,
        )
        .await;
    assert_eq!(v["participants"][0]["presence"], "connected");
}

#[tokio::test]
async fn heartbeat_and_drift_correction_over_the_wire() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    let mut b = s.connect(&room, "bob").await;
    a.send("select_media", json!({ "media": media("m") })).await;
    a.send("playback_play", json!({ "position": 100.0 })).await;
    b.expect("playback_play").await;

    // Heartbeat (1s in the test configuration) while playing.
    let hb = b.expect("sync_state").await;
    assert_eq!(hb["reason"], "heartbeat");
    assert_eq!(hb["state"], "playing");
    assert!(hb["position"].as_f64().unwrap() >= 100.0);

    // Bob is 20s behind: hard seek.
    b.send(
        "sync_report",
        json!({ "position": 80.0, "state": "playing", "rtt_ms": 40.0 }),
    )
    .await;
    let c = b.expect("sync_correction").await;
    assert_eq!(c["action"], "seek");
    assert!(c["position"].as_f64().unwrap() > 100.0);
    assert!(c["drift_ms"].as_f64().unwrap() < -1000.0);
    // Corrections are per participant.
    assert_eq!(s.state.metrics.sync_corrections_total.get(), 1);
}

#[tokio::test]
async fn paused_rooms_send_no_heartbeat() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    a.expect("chat_history").await;
    a.assert_silent(1300).await;
}

#[tokio::test]
async fn origin_header_must_be_allow_listed() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let t = token("alice");
    assert!(
        Ws::connect_with(&s, &room, Some(&t), &[("origin", "https://evil.example")])
            .await
            .is_err()
    );
    let mut ok = Ws::connect_with(&s, &room, Some(&t), &[("origin", "https://app.example")])
        .await
        .unwrap();
    ok.expect("room_state").await;
}

#[tokio::test]
async fn cors_is_not_wildcard() {
    let s = TestServer::start(&[]).await;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let preflight = |origin: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/rooms")
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "authorization")
            .body(Body::empty())
            .unwrap()
    };
    let ok = flicksync::app::build_router(s.state.clone())
        .oneshot(preflight("https://app.example"))
        .await
        .unwrap();
    assert_eq!(
        ok.headers().get("access-control-allow-origin").unwrap(),
        "https://app.example"
    );
    let bad = flicksync::app::build_router(s.state.clone())
        .oneshot(preflight("https://evil.example"))
        .await
        .unwrap();
    assert!(bad.headers().get("access-control-allow-origin").is_none());

    // FlickDD: DELETE, Range and If-Range are allowed; the download headers are exposed.
    let dd_preflight = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/downloads/x")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "DELETE")
        .header("access-control-request-headers", "range,if-range")
        .body(Body::empty())
        .unwrap();
    let ok = flicksync::app::build_router(s.state.clone())
        .oneshot(dd_preflight)
        .await
        .unwrap();
    let h = |name: &str| {
        ok.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_ascii_lowercase())
            .unwrap_or_default()
    };
    assert!(h("access-control-allow-methods").contains("delete"));
    assert!(h("access-control-allow-headers").contains("if-range"));
    let simple = Request::builder()
        .uri("/health")
        .header("origin", "https://app.example")
        .body(Body::empty())
        .unwrap();
    let resp = flicksync::app::build_router(s.state.clone())
        .oneshot(simple)
        .await
        .unwrap();
    let exposed = resp
        .headers()
        .get("access-control-expose-headers")
        .map(|v| v.to_str().unwrap().to_ascii_lowercase())
        .unwrap_or_default();
    for name in [
        "content-range",
        "etag",
        "accept-ranges",
        "content-length",
        "retry-after",
    ] {
        assert!(exposed.contains(name), "{name} not in {exposed:?}");
    }
}

#[tokio::test]
async fn token_can_be_passed_as_query_parameter_for_browser_clients() {
    let s = TestServer::start(&[]).await;
    let room = s.create_room("alice").await;
    let url = format!(
        "ws://{}/api/v1/rooms/{room}/ws?access_token={}",
        s.addr,
        token("alice")
    );
    let (mut stream, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    use futures_util::StreamExt;
    let first = stream.next().await.unwrap().unwrap();
    assert!(first.into_text().unwrap().contains("room_state"));
}

#[tokio::test]
async fn connection_limit_is_enforced() {
    let s = TestServer::start(&[("FLICKSYNC_MAX_CONNECTIONS", "1")]).await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    assert!(Ws::connect(&s, &room, &token("bob")).await.is_err());
    a.close().await;
}

#[tokio::test]
async fn idle_connections_are_dropped() {
    // No client-side pong handling in this raw client path: it simply stays silent,
    // but tungstenite answers pings automatically while polled, so use a tiny idle limit
    // together with a client that never polls.
    let s = TestServer::start(&[
        ("FLICKSYNC_WS_PING_INTERVAL", "1"),
        ("FLICKSYNC_WS_IDLE_TIMEOUT", "2"),
    ])
    .await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    // Not polling the socket means pongs are never sent back.
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    let mgr = s.state.manager.clone();
    let metrics = s.state.metrics.clone();
    eventually("server dropped idle socket", || {
        metrics.ws_connections.get() == 0
    })
    .await;
    let _ = mgr;
}

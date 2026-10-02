//! Admin API used by the web panel.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

const ADMIN: &str = "admin-token-0123456789abcdef";

async fn start() -> TestServer {
    TestServer::start(&[("FLICKSYNC_ADMIN_TOKEN", ADMIN)]).await
}

#[tokio::test]
async fn admin_api_is_disabled_without_a_token() {
    let s = TestServer::start(&[]).await;
    for path in [
        "/admin/v1/overview",
        "/admin/v1/rooms",
        "/admin/v1/stats",
        "/admin/v1/invite",
    ] {
        let (st, _) = s.http(Method::GET, path, Some(ADMIN), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn admin_api_requires_the_exact_token() {
    let s = start().await;
    for path in [
        "/admin/v1/overview",
        "/admin/v1/rooms",
        "/admin/v1/stats",
        "/admin/v1/invite",
    ] {
        let (st, _) = s.http(Method::GET, path, None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path} without token");
        let (st, _) = s
            .http(Method::GET, path, Some("wrong-token-0123456789"), None)
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path} wrong token");
        // A valid Flick user token is not an admin token.
        let (st, _) = s.http(Method::GET, path, Some(&token("alice")), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path} user token");
        let (st, _) = s.http(Method::GET, path, Some(ADMIN), None).await;
        assert_eq!(st, StatusCode::OK, "{path} admin token");
    }
    let (st, _) = s
        .http(Method::DELETE, "/admin/v1/rooms/ABCDEFGHJKMN", None, None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn overview_reports_live_numbers() {
    let s = start().await;
    let (_, o) = s
        .http(Method::GET, "/admin/v1/overview", Some(ADMIN), None)
        .await;
    assert_eq!(o["rooms"], 0);
    assert_eq!(o["ready"], true);
    s.create_room("alice").await;
    s.create_room("bob").await;
    let (_, o) = s
        .http(Method::GET, "/admin/v1/overview", Some(ADMIN), None)
        .await;
    assert_eq!(o["rooms"], 2);
    assert!(o["version"].is_string());
}

#[tokio::test]
async fn invite_returns_the_link_and_a_qr_and_is_not_cacheable() {
    let s = TestServer::start(&[
        ("FLICKSYNC_ADMIN_TOKEN", ADMIN),
        ("FLICKSYNC_PUBLIC_URL", "https://sync.example.com"),
    ])
    .await;
    let (st, inv) = s
        .http(Method::GET, "/admin/v1/invite", Some(ADMIN), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let url = inv["url"].as_str().unwrap();
    assert!(url.starts_with("flicksync://sync.example.com/?v=1&tls=1#k="));
    let parsed: flicksync::invite::Invitation = url.parse().unwrap();
    assert_eq!(parsed.endpoint.authority, "sync.example.com");
    assert_eq!(inv["address"], "https://sync.example.com");
    assert_eq!(inv["address_guessed"], false);

    // A path prefix (reverse proxy) ends up in the link and in the address.
    let p = TestServer::start(&[
        ("FLICKSYNC_ADMIN_TOKEN", ADMIN),
        ("FLICKSYNC_PUBLIC_URL", "https://flick.example.com/sync"),
    ])
    .await;
    let (_, pinv) = p
        .http(Method::GET, "/admin/v1/invite", Some(ADMIN), None)
        .await;
    assert!(
        pinv["url"]
            .as_str()
            .unwrap()
            .starts_with("flicksync://flick.example.com/sync/?v=1&tls=1#k=")
    );
    assert_eq!(pinv["address"], "https://flick.example.com/sync");
    assert_eq!(inv["key_source"], "environment");
    assert!(inv["qr"].as_array().unwrap().len() > 20);

    // A guessed address is flagged.
    let g = start().await;
    let (_, inv) = g
        .http(Method::GET, "/admin/v1/invite", Some(ADMIN), None)
        .await;
    assert_eq!(inv["address_guessed"], true);

    // no-store
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let resp = flicksync::app::build_router(s.state.clone())
        .oneshot(
            Request::builder()
                .uri("/admin/v1/invite")
                .header("authorization", format!("Bearer {ADMIN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn rooms_list_shows_participants_and_media() {
    let s = start().await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    a.expect("chat_history").await;
    a.send(
        "select_media",
        json!({ "media": { "provider": "jellyfin", "server_id": "jf", "media_id": "m1", "media_type": "movie", "title": "The Long Night", "duration_secs": 5400.0 } }),
    )
    .await;
    a.expect("media_selected").await;

    let (st, body) = s
        .http(Method::GET, "/admin/v1/rooms", Some(ADMIN), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let rooms = body["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1);
    let r = &rooms[0];
    assert_eq!(r["room_id"], room);
    assert!(r["share_code"].as_str().unwrap().contains('-'));
    assert_eq!(r["state"], "media_selected");
    assert_eq!(r["media_title"], "The Long Night");
    assert_eq!(r["media_provider"], "jellyfin");
    assert_eq!(r["host_id"], "alice");
    assert_eq!(r["participants"][0]["presence"], "connected");
    assert_eq!(r["participants"][0]["is_host"], true);
    assert!(r["idle_secs"].as_u64().unwrap() < 5);
}

#[tokio::test]
async fn deleting_a_room_closes_it_for_everybody() {
    let s = start().await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    a.expect("chat_history").await;
    let mut b = s.connect(&room, "bob").await;
    b.expect("room_state").await;

    let (st, _) = s
        .http(
            Method::DELETE,
            &format!("/admin/v1/rooms/{room}"),
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    let closed = a.expect("room_closed").await;
    assert_eq!(closed["reason"], "admin_closed");
    let closed = b.expect("room_closed").await;
    assert_eq!(closed["reason"], "admin_closed");

    let (_, body) = s
        .http(Method::GET, "/admin/v1/rooms", Some(ADMIN), None)
        .await;
    assert!(body["rooms"].as_array().unwrap().is_empty());
    let (st, _) = s
        .http(
            Method::DELETE,
            &format!("/admin/v1/rooms/{room}"),
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = s
        .http(
            Method::DELETE,
            "/admin/v1/rooms/not-a-room",
            Some(ADMIN),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stats_expose_counters_drift_buckets_and_history() {
    let s = start().await;
    let room = s.create_room("alice").await;
    let mut a = s.connect(&room, "alice").await;
    a.expect("room_state").await;
    a.expect("chat_history").await;
    a.send(
        "select_media",
        json!({ "media": { "provider": "plex", "server_id": "p", "media_id": "m", "media_type": "movie" } }),
    )
    .await;
    a.expect("media_selected").await;
    a.send("playback_play", json!({ "position": 0.0 })).await;
    a.expect("playback_play").await;
    a.send(
        "sync_report",
        json!({ "position": 0.0, "state": "playing", "rtt_ms": 20.0 }),
    )
    .await;
    // Reports get no direct reply when in sync: use a ping round trip as a barrier.
    a.send("ping", json!({ "client_time": 1 })).await;
    a.expect("pong").await;

    let (st, stats) = s
        .http(Method::GET, "/admin/v1/stats", Some(ADMIN), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(stats["totals"]["rooms_created"], 1);
    assert!(stats["totals"]["messages_in"].as_u64().unwrap() >= 4);
    assert_eq!(stats["totals"]["sync_reports"], 1);
    let buckets: u64 = stats["drift"]["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap())
        .sum();
    assert_eq!(buckets, 1);
    assert_eq!(stats["drift"]["thresholds_ms"]["hard"], 1500.0);
    assert_eq!(stats["history_interval_secs"], 10);
    assert!(stats["history"].is_array());
}

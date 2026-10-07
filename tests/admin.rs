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
    assert!(url.starts_with("flickserver://sync.example.com/?v=1&tls=1#k="));
    let parsed: flicksync::invite::Invitation = url.parse().unwrap();
    assert_eq!(parsed.endpoint.authority, "sync.example.com");
    assert_eq!(inv["address"], "https://sync.example.com");
    assert_eq!(inv["address_guessed"], false);

    // A path prefix (reverse proxy) ends up in the link and in the address.
    let p = TestServer::start(&[
        ("FLICKSYNC_ADMIN_TOKEN", ADMIN),
        ("FLICKSYNC_PUBLIC_URL", "https://flick.example.com/services"),
    ])
    .await;
    let (_, pinv) = p
        .http(Method::GET, "/admin/v1/invite", Some(ADMIN), None)
        .await;
    assert!(
        pinv["url"]
            .as_str()
            .unwrap()
            .starts_with("flickserver://flick.example.com/services/?v=1&tls=1#k=")
    );
    assert_eq!(pinv["address"], "https://flick.example.com/services");
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

// ---------------------------------------------------------------- FlickDD

mod dd_admin {
    use std::time::Duration;

    use super::*;
    use common::fake_media::ITEM;
    use flicksync::dd::now_ms;

    const MIB: u64 = 1024 * 1024;
    const ROUTES: [&str; 4] = [
        "/admin/v1/dd/overview",
        "/admin/v1/dd/active",
        "/admin/v1/dd/history",
        "/admin/v1/dd/stats",
    ];

    /// FlickDD enabled and an admin token; the fake media server is kept alive by leaking it.
    async fn start_dd_admin(size: u64, extra: &[(&str, &str)]) -> TestServer {
        let mut vars = vec![("FLICKSYNC_ADMIN_TOKEN", ADMIN)];
        vars.extend_from_slice(extra);
        let (fake, server) = start_dd(size, &vars).await;
        std::mem::forget(fake);
        server
    }

    struct Client {
        id: String,
        token: String,
    }

    async fn create(s: &TestServer, user: &str) -> Client {
        let jwt = token_for(user, SERVER, "k1", SECRET, &["downloads:create"]);
        let body = json!({
            "backend": "jellyfin", "item_id": ITEM, "title": "Movie\u{7}One", "kind": "movie"
        });
        let (st, j) = s
            .http(Method::POST, "/api/v1/downloads", Some(&jwt), Some(body))
            .await;
        assert_eq!(st, StatusCode::CREATED, "{j}");
        Client {
            id: j["download_id"].as_str().unwrap().to_owned(),
            token: j["token"].as_str().unwrap().to_owned(),
        }
    }

    #[tokio::test]
    async fn dd_routes_need_the_admin_token() {
        let s = start_dd_admin(MIB, &[]).await;
        let mut calls: Vec<(Method, String)> = ROUTES
            .iter()
            .map(|r| (Method::GET, (*r).to_owned()))
            .collect();
        calls.push((Method::DELETE, "/admin/v1/dd/abc".to_owned()));
        for (m, path) in &calls {
            let (st, _) = s.http(m.clone(), path, None, None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{path} no token");
            let (st, _) = s.http(m.clone(), path, Some(&token("alice")), None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{path} user token");
        }
        for path in ROUTES {
            let (st, _) = s.http(Method::GET, path, Some(ADMIN), None).await;
            assert_eq!(st, StatusCode::OK, "{path}");
        }
        let (st, j) = s
            .http(Method::DELETE, "/admin/v1/dd/nope", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(j["error"]["code"], "DOWNLOAD_NOT_FOUND", "{j}");
    }

    #[tokio::test]
    async fn dd_routes_are_404_without_an_admin_token_configured() {
        let (_fake, s) = start_dd(MIB, &[]).await;
        for path in ROUTES {
            let (st, _) = s.http(Method::GET, path, Some(ADMIN), None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        }
        let (st, _) = s
            .http(Method::DELETE, "/admin/v1/dd/x", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dd_routes_are_404_when_flickdd_is_disabled() {
        let s = start().await;
        for path in ROUTES {
            let (st, _) = s.http(Method::GET, path, Some(ADMIN), None).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        }
        let (st, _) = s
            .http(Method::DELETE, "/admin/v1/dd/x", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn overview_reports_limits_and_backends() {
        let s = start_dd_admin(MIB, &[]).await;
        let (st, o) = s
            .http(Method::GET, "/admin/v1/dd/overview", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(o["enabled"], true);
        assert_eq!(o["backends"], json!({"jellyfin": true, "plex": true}));
        assert_eq!(o["limits"]["max_parallel"], 10);
        assert_eq!(o["limits"]["max_global"], 100);
        assert_eq!(o["limits"]["rate_bps"], 10 * MIB);
        assert_eq!(o["limits"]["chunk_bytes"], 8 * MIB);
        assert_eq!(o["limits"]["max_range_bytes"], 64 * MIB);
        assert_eq!(o["limits"]["grant_ttl_secs"], 21_600);
        assert_eq!(o["active"], 0);
        assert_eq!(o["totals"]["downloads"], 0);
        create(&s, "alice").await;
        let (_, o) = s
            .http(Method::GET, "/admin/v1/dd/overview", Some(ADMIN), None)
            .await;
        assert_eq!(o["active"], 1);
    }

    #[tokio::test]
    async fn active_then_cancel_moves_to_history_and_ends_the_stream() {
        let s = start_dd_admin(16 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
        let c = create(&s, "alice").await;
        let mut resp = reqwest::Client::new()
            .get(format!("http://{}/api/v1/downloads/{}/file", s.addr, c.id))
            .bearer_auth(&c.token)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.chunk().await.unwrap().is_some());
        // Paused from here on: nobody reads, the pump blocks on its channel.

        let dd = s.state.dd().unwrap();
        eventually("streaming", || {
            dd.grants.active_views(now_ms()).iter().any(|v| v.streaming)
        })
        .await;
        let (st, a) = s
            .http(Method::GET, "/admin/v1/dd/active", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(a["now"].as_u64().unwrap() > 0);
        assert_eq!(a["downloads"].as_array().unwrap().len(), 1, "{a}");
        let d = &a["downloads"][0];
        assert_eq!(d["download_id"], c.id.as_str());
        assert_eq!(d["user_id"], format!("{SERVER}/alice"));
        assert_eq!(d["size"], 16 * MIB);
        assert!(d["covered"].as_u64().is_some(), "{d}");
        assert_eq!(d["streaming"], true);

        let path = format!("/admin/v1/dd/{}", c.id);
        let (st, _) = s.http(Method::DELETE, &path, Some(ADMIN), None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);

        // The client sees the stream end early.
        let mut got = 0u64;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), resp.chunk()).await {
                Err(_) => panic!("stream still open after the admin cancel"),
                Ok(Ok(Some(b))) => got += b.len() as u64,
                Ok(_) => break,
            }
        }
        assert!(got < 16 * MIB, "{got}");

        let (_, a) = s
            .http(Method::GET, "/admin/v1/dd/active", Some(ADMIN), None)
            .await;
        assert_eq!(a["downloads"].as_array().unwrap().len(), 0);
        let (st, h) = s
            .http(Method::GET, "/admin/v1/dd/history", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h["downloads"][0]["download_id"], c.id.as_str());
        assert_eq!(h["downloads"][0]["outcome"], "cancelled");
        assert!(h["now"].as_u64().is_some());

        let (st, _) = s.http(Method::DELETE, &path, Some(ADMIN), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "already cancelled");
    }

    #[tokio::test]
    async fn stats_has_totals_days_and_top_titles() {
        let s = start_dd_admin(MIB, &[]).await;
        let (st, j) = s
            .http(Method::GET, "/admin/v1/dd/stats", Some(ADMIN), None)
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(j["now"].as_u64().is_some());
        assert!(j["totals"].is_object(), "{j}");
        assert!(j["days"].is_array(), "{j}");
        assert!(j["top_titles"].is_array(), "{j}");
    }

    /// Raw request against the live server, to inspect headers.
    async fn raw(
        s: &TestServer,
        m: reqwest::Method,
        path: &str,
        tok: Option<&str>,
    ) -> reqwest::Response {
        let mut r = reqwest::Client::new().request(m, format!("http://{}{path}", s.addr));
        if let Some(t) = tok {
            r = r.bearer_auth(t);
        }
        r.send().await.unwrap()
    }

    fn no_store(r: &reqwest::Response, what: &str) {
        let h = r
            .headers()
            .get("cache-control")
            .map(|v| v.to_str().unwrap());
        assert_eq!(h, Some("no-store"), "{what} ({})", r.status());
    }

    #[tokio::test]
    async fn every_dd_response_is_no_store_including_errors() {
        let s = start_dd_admin(MIB, &[]).await;
        let c = create(&s, "alice").await;
        for path in ROUTES {
            let r = raw(&s, reqwest::Method::GET, path, Some(ADMIN)).await;
            assert_eq!(r.status().as_u16(), 200);
            no_store(&r, path);
            let r = raw(&s, reqwest::Method::GET, path, None).await;
            assert_eq!(r.status().as_u16(), 401);
            no_store(&r, &format!("{path} 401"));
        }
        let missing = raw(
            &s,
            reqwest::Method::DELETE,
            "/admin/v1/dd/nope",
            Some(ADMIN),
        )
        .await;
        assert_eq!(missing.status().as_u16(), 404);
        no_store(&missing, "DELETE 404");
        let del = raw(
            &s,
            reqwest::Method::DELETE,
            &format!("/admin/v1/dd/{}", c.id),
            Some(ADMIN),
        )
        .await;
        assert_eq!(del.status().as_u16(), 204);
        no_store(&del, "DELETE 204");
        let r = raw(&s, reqwest::Method::DELETE, "/admin/v1/dd/x", None).await;
        assert_eq!(r.status().as_u16(), 401);
        no_store(&r, "DELETE 401");
    }

    #[tokio::test]
    async fn disabled_dd_is_404_with_no_store_even_with_a_valid_admin_token() {
        let s = start().await;
        assert!(s.state.dd().is_none());
        let r = raw(
            &s,
            reqwest::Method::GET,
            "/admin/v1/dd/overview",
            Some(ADMIN),
        )
        .await;
        assert_eq!(r.status().as_u16(), 404);
        no_store(&r, "disabled");
        let j: serde_json::Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(j["error"]["code"], "DOWNLOAD_NOT_FOUND");
    }

    #[tokio::test]
    async fn grant_tokens_never_appear_in_admin_json() {
        let s = start_dd_admin(MIB, &[]).await;
        let c = create(&s, "alice").await;
        let jwt = token_for("alice", SERVER, "k1", SECRET, &["downloads:create"]);
        let mut paths: Vec<String> = ROUTES.iter().map(|r| (*r).to_owned()).collect();
        paths.extend(ROUTES.iter().map(|r| format!("{r}?token={}", c.token)));
        // Live grant, then after it is cancelled (history).
        for round in 0..2 {
            for path in &paths {
                let (st, body) = s.http(Method::GET, path, Some(ADMIN), None).await;
                assert_eq!(st, StatusCode::OK, "{path}");
                let text = body.to_string();
                assert!(!text.contains(&c.token), "{path} leaks the grant token");
                assert!(!text.contains(&jwt), "{path} leaks the user JWT");
                assert!(!text.contains("token_hash"), "{path}");
            }
            if round == 0 {
                let (st, _) = s
                    .http(
                        Method::DELETE,
                        &format!("/admin/v1/dd/{}", c.id),
                        Some(ADMIN),
                        None,
                    )
                    .await;
                assert_eq!(st, StatusCode::NO_CONTENT);
            }
        }
        let (_, h) = s
            .http(Method::GET, "/admin/v1/dd/history", Some(ADMIN), None)
            .await;
        assert_eq!(h["downloads"][0]["download_id"], c.id.as_str());
        // The same goes for the error body of a bad cancel.
        let (_, e) = s
            .http(
                Method::DELETE,
                &format!("/admin/v1/dd/{}?token={}", c.id, c.token),
                Some(ADMIN),
                None,
            )
            .await;
        assert!(!e.to_string().contains(&c.token));
    }
}

#[tokio::test]
async fn the_admin_api_accepts_the_token_derived_from_the_panel_password() {
    let password = "correct horse battery staple";
    let derived = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";
    let s = TestServer::start(&[("PANEL_PASSWORD", password)]).await;
    let (st, _) = s
        .http(Method::GET, "/admin/v1/overview", Some(derived), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = s
        .http(
            Method::GET,
            "/admin/v1/overview",
            Some("wrong-token-0123456789"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // The password itself is not the token.
    let (st, _) = s
        .http(Method::GET, "/admin/v1/overview", Some(password), None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_a_long_enough_password_or_legacy_token_the_admin_api_is_off() {
    let cases: [&[(&str, &str)]; 2] = [&[], &[("PANEL_PASSWORD", "short")]];
    for vars in cases {
        let s = TestServer::start(vars).await;
        let (st, _) = s
            .http(
                Method::GET,
                "/admin/v1/overview",
                Some("x".repeat(40).as_str()),
                None,
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}

//! Module lifecycle: start, stop and reload at runtime.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use flicksync::modules::ModuleId;
use flicksync::settings::store::Scope;
use std::collections::BTreeMap;

fn patch(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), Some(v.to_string())))
        .collect()
}

#[tokio::test]
async fn a_stopped_module_answers_503_and_the_server_stays_up() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let (st, body) = s
        .http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None)
        .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "MODULE_DISABLED");
    assert_eq!(
        s.http(Method::GET, "/health", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        s.http(Method::GET, "/ready", None, None).await.0,
        StatusCode::OK
    );
    // FlickDD keeps its documented 404.
    let (st, _) = s
        .http(
            Method::POST,
            "/api/v1/downloads",
            Some(&token("alice")),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let m = s.state.module_status(ModuleId::FlickSync);
    assert_eq!((m.state, m.enabled), ("stopped", false));
}

#[tokio::test]
async fn start_serves_and_stop_closes_open_rooms() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let m = s.state.start_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("running", true));
    let room = s.create_room("alice").await;
    let mut ws = s.connect(&room, "alice").await;
    ws.expect("room_state").await;

    let m = s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("stopped", false));
    ws.expect_closed().await;
    let (st, body) = s
        .http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None)
        .await;
    assert_eq!(
        (st, body["error"]["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("MODULE_DISABLED"))
    );
}

#[tokio::test]
async fn start_and_stop_are_idempotent() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    s.state.start_module(ModuleId::FlickSync).unwrap();
    let first = s.state.sync().unwrap();
    s.state.start_module(ModuleId::FlickSync).unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&first, &s.state.sync().unwrap()),
        "no second runtime"
    );
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert!(s.state.sync().is_err());
}

#[tokio::test]
async fn reload_picks_up_new_settings_and_reports_pending_changes() {
    let s = TestServer::start(&[]).await;
    assert!(!s.state.module_status(ModuleId::FlickSync).pending_reload);
    s.state
        .settings
        .put(Scope::FlickSync, patch(&[("FLICKSYNC_MAX_ROOM_SIZE", "1")]))
        .unwrap();
    assert!(s.state.module_status(ModuleId::FlickSync).pending_reload);

    // Still the old limit until reloaded.
    let old_room = s.create_room("alice").await;
    assert_eq!(s.join_http(&old_room, "bob").await.0, StatusCode::OK);

    let m = s.state.reload_module(ModuleId::FlickSync);
    assert_eq!((m.state, m.pending_reload), ("running", false));
    let room = s.create_room("alice").await;
    assert_eq!(
        s.join_http(&room, "bob").await.0,
        StatusCode::CONFLICT,
        "room size 1 after reload"
    );
}

#[tokio::test]
async fn reload_never_starts_a_disabled_module() {
    let s = TestServer::start(&[("FLICKSYNC_ENABLED", "false")]).await;
    let m = s.state.reload_module(ModuleId::FlickSync);
    assert_eq!(m.state, "stopped");
}

#[tokio::test]
async fn a_failing_start_leaves_the_module_failed_and_the_rest_running() {
    let s = TestServer::start(&[]).await;
    // FlickDD enabled but no backend configured anywhere.
    let m = s.state.start_module(ModuleId::FlickDd).unwrap();
    assert_eq!(m.state, "failed");
    assert!(m.message.unwrap().contains("backend"), "says why");
    assert!(m.enabled, "the intent is kept");
    assert!(s.state.sync().is_ok(), "FlickSync is untouched");
    assert_eq!(
        s.http(Method::GET, "/health", None, None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_request_in_flight_survives_a_stop() {
    let s = TestServer::start(&[]).await;
    let _room = s.create_room("alice").await;
    let held = s.state.sync().unwrap(); // what a handler keeps for the whole request
    assert_eq!(held.manager.room_count(), 1);
    s.state.stop_module(ModuleId::FlickSync).unwrap();
    assert_eq!(
        held.manager.room_count(),
        0,
        "the old runtime is still usable and closed"
    );
    assert!(!held.manager.is_accepting());
    assert!(s.state.sync().is_err(), "new requests are refused");
}

#[test]
fn concurrent_lifecycle_calls_are_serialised() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = rt.block_on(TestServer::start(&[]));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let state = s.state.clone();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    match i % 3 {
                        0 => {
                            state.reload_module(ModuleId::FlickSync);
                        }
                        1 => {
                            state.stop_module(ModuleId::FlickSync).unwrap();
                        }
                        _ => {
                            state.start_module(ModuleId::FlickSync).unwrap();
                        }
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    // The runtime and the persisted switch must agree, whatever the interleaving was.
    let m = s.state.module_status(ModuleId::FlickSync);
    assert_eq!(
        m.state == "running",
        m.enabled,
        "runtime and persisted switch disagree"
    );
    // One more start settles it.
    let m = s.state.start_module(ModuleId::FlickSync).unwrap();
    assert_eq!((m.state, m.enabled), ("running", true));
    let a = s.state.sync().unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &s.state.sync().unwrap()));
}

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

async fn preflight(s: &TestServer, origin: &str) -> Option<String> {
    let resp = flicksync::app::build_router(s.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/v1/rooms")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.headers()
        .get("access-control-allow-origin")
        .map(|v| v.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn a_server_reload_changes_cors_origins_for_new_requests() {
    let s = TestServer::start(&[]).await;
    assert_eq!(
        preflight(&s, "https://app.example").await.as_deref(),
        Some("https://app.example")
    );
    s.state
        .settings
        .put(
            Scope::Server,
            patch(&[("FLICKSYNC_CORS_ORIGINS", "https://new.example")]),
        )
        .unwrap();
    assert_eq!(
        preflight(&s, "https://app.example").await.as_deref(),
        Some("https://app.example"),
        "not before the reload"
    );
    s.state.reload_server().unwrap();
    assert_eq!(preflight(&s, "https://app.example").await, None);
    assert_eq!(
        preflight(&s, "https://new.example").await.as_deref(),
        Some("https://new.example")
    );
}

#[tokio::test]
async fn a_server_reload_swaps_the_signing_keys() {
    let s = TestServer::start(&[]).await;
    let k1 = token_for("alice", SERVER, "k1", SECRET, &["*"]);
    let k2 = token_for("alice", OTHER_SERVER, "k2", OTHER_SECRET, &["*"]);
    assert_eq!(
        s.http(Method::POST, "/api/v1/rooms", Some(&k1), None)
            .await
            .0,
        StatusCode::CREATED
    );

    // Keep only k2.
    s.state
        .settings
        .put(
            Scope::Server,
            patch(&[(
                "FLICKSYNC_AUTH_KEYS",
                &format!("k2:{OTHER_SERVER}:{OTHER_SECRET}"),
            )]),
        )
        .unwrap();
    s.state.reload_server().unwrap();
    assert_eq!(
        s.http(Method::POST, "/api/v1/rooms", Some(&k1), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "removed key"
    );
    assert_eq!(
        s.http(Method::POST, "/api/v1/rooms", Some(&k2), None)
            .await
            .0,
        StatusCode::CREATED,
        "kept key"
    );
}

#[tokio::test]
async fn a_malformed_key_list_is_refused_and_the_old_keys_stay() {
    let s = TestServer::start(&[]).await;
    assert!(
        s.state
            .settings
            .put(
                Scope::Server,
                patch(&[("FLICKSYNC_AUTH_KEYS", "not-a-key")])
            )
            .is_err()
    );
    s.state.reload_server().unwrap();
    assert_eq!(
        s.http(Method::POST, "/api/v1/rooms", Some(&token("alice")), None)
            .await
            .0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn a_server_reload_applies_the_log_level() {
    let s = TestServer::start(&[]).await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = seen.clone();
    s.state
        .set_log_control(std::sync::Arc::new(move |level: &str| {
            sink.lock().unwrap().push(level.to_owned());
            Ok(())
        }));
    s.state
        .settings
        .put(Scope::Server, patch(&[("FLICKSYNC_LOG_LEVEL", "debug")]))
        .unwrap();
    s.state.reload_server().unwrap();
    assert_eq!(seen.lock().unwrap().as_slice(), ["debug"]);
}

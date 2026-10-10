//! The in-memory log buffer: wiring into the state, and `GET /admin/v1/logs`.

mod common;

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use common::*;
use flicksync::app::AppState;
use flicksync::logs::{Level, LogBuffer, LogCapture};
use flicksync::settings::{Env, Settings};
use serde_json::Value;

const ADMIN: &str = "admin-token-0123456789abcdef";

async fn start(extra: &[(&str, &str)]) -> TestServer {
    let mut vars = vec![("FLICKSYNC_ADMIN_TOKEN", ADMIN)];
    vars.extend_from_slice(extra);
    TestServer::start(&vars).await
}

async fn logs(s: &TestServer, query: &str) -> (StatusCode, Value) {
    s.http(
        Method::GET,
        &format!("/admin/v1/logs{query}"),
        Some(ADMIN),
        None,
    )
    .await
}

fn messages(body: &Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["message"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn the_state_serves_the_buffer_main_feeds() {
    let s = TestServer::start(&[("FLICKSYNC_LOG_BUFFER", "100")]).await;
    assert_eq!(s.state.logs.capacity(), 100);

    let dir = flicksync::settings::scratch_dir("logs");
    let data = dir.to_str().unwrap().to_owned();
    let env: Env = Arc::new(move |k: &str| match k {
        "FLICKSYNC_AUTH_KEYS" => Some(format!("k1:{SERVER}:{SECRET}")),
        "FLICKSYNC_DATA_DIR" => Some(data.clone()),
        _ => None,
    });
    let logs = Arc::new(LogBuffer::new(7));
    let state =
        AppState::with_log_buffer(Settings::open(&dir, env).unwrap(), logs.clone()).unwrap();
    assert!(Arc::ptr_eq(&state.logs, &logs));
}

#[tokio::test]
async fn the_logs_need_the_admin_token() {
    let s = start(&[]).await;
    let (st, _) = s.http(Method::GET, "/admin/v1/logs", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = s
        .http(Method::GET, "/admin/v1/logs", Some(&token("alice")), None)
        .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "a user token is not an admin token"
    );
    let (st, body) = logs(&s, "").await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let off = TestServer::start(&[]).await;
    let (st, _) = off
        .http(Method::GET, "/admin/v1/logs", Some(ADMIN), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "admin API off");
}

#[tokio::test]
async fn every_logs_answer_is_no_store() {
    let s = start(&[]).await;
    for (query, token, status) in [
        ("", Some(ADMIN), 200),
        ("?limit=0", Some(ADMIN), 400),
        ("", None, 401),
    ] {
        let mut req = reqwest::Client::new().get(format!("http://{}/admin/v1/logs{query}", s.addr));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let r = req.send().await.unwrap();
        assert_eq!(r.status().as_u16(), status, "{query}");
        assert_eq!(
            r.headers()
                .get("cache-control")
                .map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{query} ({status})"
        );
    }
}

#[tokio::test]
async fn lines_come_back_after_the_cursor_page_by_page() {
    let s = start(&[]).await;
    let l = &s.state.logs;
    l.push(
        Level::Info,
        "flicksync::room",
        "room created",
        "room_id=\"R1\"",
    );
    l.push(Level::Debug, "flicksync::room", "sync correction", "");
    l.push(
        Level::Warn,
        "flicksync::api::admin",
        "admin API: invalid or missing token",
        "",
    );

    let (st, body) = logs(&s, "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        messages(&body),
        [
            "room created",
            "sync correction",
            "admin API: invalid or missing token"
        ]
    );
    let e = &body["entries"][0];
    assert_eq!(e["seq"], 1);
    assert_eq!(e["level"], "info");
    assert_eq!(e["target"], "flicksync::room");
    assert_eq!(e["fields"], "room_id=\"R1\"");
    assert!(e["ts"].as_u64().unwrap() > 0);
    assert_eq!(body["next"], 3);
    assert_eq!(body["more"], false);
    assert_eq!(body["dropped"], 0);
    assert_eq!(body["capacity"], 2000);
    assert_eq!(body["log_level"], "info");
    assert_eq!(body["boot"].as_u64(), Some(s.state.logs.boot()));

    let (_, first) = logs(&s, "?limit=2").await;
    assert_eq!(messages(&first), ["room created", "sync correction"]);
    assert_eq!(first["next"], 2);
    assert_eq!(first["more"], true);
    let (_, rest) = logs(&s, "?after=2&limit=2").await;
    assert_eq!(messages(&rest), ["admin API: invalid or missing token"]);
    assert_eq!(rest["more"], false);

    let (_, warn) = logs(&s, "?level=WARN").await;
    assert_eq!(messages(&warn), ["admin API: invalid or missing token"]);
    let (_, none) = logs(&s, "?after=3").await;
    assert!(none["entries"].as_array().unwrap().is_empty());
    assert_eq!(none["next"], 3);
}

#[tokio::test]
async fn a_cursor_that_fell_off_the_buffer_reports_the_lost_lines() {
    let s = start(&[("FLICKSYNC_LOG_BUFFER", "100")]).await;
    for i in 0..150 {
        s.state
            .logs
            .push(Level::Info, "t", &format!("line {i}"), "");
    }
    let (_, body) = logs(&s, "?after=10&limit=1000").await;
    assert_eq!(body["capacity"], 100);
    assert_eq!(body["dropped"], 40);
    assert_eq!(body["entries"][0]["seq"], 51);
    assert_eq!(body["entries"].as_array().unwrap().len(), 100);
}

#[tokio::test]
async fn bad_queries_are_refused_with_invalid_query() {
    let s = start(&[]).await;
    for (query, needle) in [
        ("?after=abc", "after must be a whole number"),
        ("?after=-1", "after must be a whole number"),
        (
            "?level=loud",
            "level must be one of error, warn, info, debug, trace",
        ),
        ("?limit=0", "from 1 to 1000"),
        ("?limit=1001", "from 1 to 1000"),
        ("?since=5", "Unknown query parameter 'since'"),
        ("?after=1&after=2", "given twice"),
    ] {
        let (st, body) = logs(&s, query).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(body["error"]["code"], "INVALID_QUERY", "{query}");
        assert!(
            body["error"]["message"].as_str().unwrap().contains(needle),
            "{query}: {body}"
        );
    }
}

#[tokio::test]
async fn secret_values_never_appear_in_the_answer() {
    let s = start(&[]).await;
    s.state.logs.push(
        Level::Warn,
        "t",
        "upgrade ?access_token=eyJhbGciOi.secret-part&x=1 with Bearer tok-123",
        "api_key=jf-key-1 password=\"hunter2\"",
    );
    let (_, body) = logs(&s, "").await;
    let text = body.to_string();
    for secret in [
        "eyJhbGciOi",
        "secret-part",
        "tok-123",
        "jf-key-1",
        "hunter2",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    assert!(text.contains("<redacted>"));
}

#[tokio::test]
async fn a_real_event_reaches_the_route_through_the_capture_layer() {
    use tracing_subscriber::layer::SubscriberExt;
    let s = start(&[]).await;
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(LogCapture::new(s.state.logs.clone()));
    // `#[tokio::test]` runs on one thread, so the router's handlers log through this subscriber.
    let _guard = tracing::subscriber::set_default(subscriber);
    let (st, _) = s
        .http(
            Method::GET,
            "/admin/v1/overview",
            Some("wrong-token-0123456789"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (_, body) = logs(&s, "?level=warn").await;
    let e = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["message"] == "admin API: invalid or missing token")
        .unwrap_or_else(|| panic!("not captured: {body}"))
        .clone();
    assert_eq!(e["level"], "warn");
    assert_eq!(e["target"], "flicksync::api::admin");
}

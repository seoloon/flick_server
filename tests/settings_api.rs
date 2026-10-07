//! Admin API: module lifecycle and settings.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{Value, json};

const PASSWORD: &str = "correct horse battery staple";
const ADMIN: &str = "602faf385fbedfdd2399872d5e5ec4359027f10a59806d0d6c4b598faf591011";

async fn server(extra: &[(&str, &str)]) -> TestServer {
    let mut vars = vec![("PANEL_PASSWORD", PASSWORD)];
    vars.extend_from_slice(extra);
    TestServer::start(&vars).await
}

async fn call(
    s: &TestServer,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    s.http(method, path, Some(ADMIN), body).await
}

fn field<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == name)
        .unwrap()
}

#[tokio::test]
async fn modules_can_be_listed_started_stopped_and_reloaded() {
    let s = server(&[("FLICKSYNC_ENABLED", "false")]).await;
    let (st, body) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert_eq!(st, StatusCode::OK);
    let states: Vec<_> = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["id"].as_str().unwrap(), m["state"].as_str().unwrap()))
        .collect();
    assert_eq!(states, [("flicksync", "stopped"), ("flickdd", "stopped")]);

    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/start", None).await;
    assert_eq!(
        (st, m["state"].as_str(), m["enabled"].as_bool()),
        (StatusCode::OK, Some("running"), Some(true))
    );
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/reload", None).await;
    assert_eq!(m["state"], "running");
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/stop", None).await;
    assert_eq!(
        (m["state"].as_str(), m["enabled"].as_bool()),
        (Some("stopped"), Some(false))
    );

    assert_eq!(
        call(&s, Method::POST, "/admin/v1/modules/nope/start", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &s,
            Method::POST,
            "/admin/v1/modules/flicksync/explode",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_module_routes_need_the_admin_token() {
    let s = server(&[]).await;
    let (st, _) = s.http(Method::GET, "/admin/v1/modules", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = s
        .http(
            Method::PUT,
            "/admin/v1/settings/flickdd",
            Some("wrong-token-0123456789"),
            Some(json!({"values": {}})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn settings_are_read_with_their_source_and_default() {
    let s = server(&[("FLICKSYNC_MAX_ROOM_SIZE", "5")]).await;
    let (st, v) = call(&s, Method::GET, "/admin/v1/settings/flicksync", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["scope"], "flicksync");
    let f = field(&v, "FLICKSYNC_MAX_ROOM_SIZE");
    assert_eq!(
        (
            f["value"].as_str(),
            f["source"].as_str(),
            f["default"].as_str()
        ),
        (Some("5"), Some("environment"), Some("100"))
    );
    assert_eq!(field(&v, "FLICKSYNC_MAX_ROOMS")["source"], "default");
    assert_eq!(
        call(&s, Method::GET, "/admin/v1/settings/nope", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_put_changes_the_value_and_flags_a_pending_reload() {
    let s = server(&[]).await;
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flicksync",
        Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 7, "FLICKSYNC_CHAT_ENABLED": false}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (
            field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["value"].as_str(),
            field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["source"].as_str()
        ),
        (Some("7"), Some("panel"))
    );
    assert_eq!(field(&v, "FLICKSYNC_CHAT_ENABLED")["value"], "false");
    let (_, mods) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert_eq!(mods["modules"][0]["pending_reload"], true);
    // null removes the stored value again.
    let (_, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flicksync",
        Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": null}})),
    )
    .await;
    assert_eq!(field(&v, "FLICKSYNC_MAX_ROOM_SIZE")["source"], "default");
}

#[tokio::test]
async fn an_invalid_put_is_refused_and_writes_nothing() {
    let s = server(&[]).await;
    let before = s.state.settings.revision();
    for body in [
        json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 0}}),
        json!({"values": {"NOPE": 1}}),
        json!({"values": {"FLICKDD_MAX_PARALLEL": 1}}),
        json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": [1]}}),
        json!({"nothing": true}),
    ] {
        let (st, v) = call(
            &s,
            Method::PUT,
            "/admin/v1/settings/flicksync",
            Some(body.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["error"]["code"], "INVALID_PAYLOAD");
    }
    assert_eq!(s.state.settings.revision(), before, "nothing was written");
}

#[tokio::test]
async fn secrets_are_write_only() {
    let s = server(&[]).await;
    let put = |values: Value| {
        call(
            &s,
            Method::PUT,
            "/admin/v1/settings/flickdd",
            Some(json!({"values": values})),
        )
    };

    let (st, v) = put(json!({"FLICKDD_JELLYFIN_URL": "http://jf:8096", "FLICKDD_JELLYFIN_API_KEY": "top-secret-jf-key"})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        !v.to_string().contains("top-secret-jf-key"),
        "the PUT answer"
    );
    let key = field(&v, "FLICKDD_JELLYFIN_API_KEY");
    assert_eq!(
        (
            key["secret"].as_bool(),
            key["set"].as_bool(),
            key["value"].is_null()
        ),
        (Some(true), Some(true), true)
    );
    let (_, got) = call(&s, Method::GET, "/admin/v1/settings/flickdd", None).await;
    assert!(
        !got.to_string().contains("top-secret-jf-key"),
        "the GET answer"
    );

    // Omitted: kept. The module starts with the stored key.
    let (st, _) = put(json!({"FLICKDD_MAX_PARALLEL": 3})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        s.state
            .settings
            .lookup("FLICKDD_JELLYFIN_API_KEY")
            .as_deref(),
        Some("top-secret-jf-key")
    );

    // A string replaces it, null clears it.
    put(json!({"FLICKDD_JELLYFIN_API_KEY": "second-secret"})).await;
    assert_eq!(
        s.state
            .settings
            .lookup("FLICKDD_JELLYFIN_API_KEY")
            .as_deref(),
        Some("second-secret")
    );
    let (st, _) = put(json!({"FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "the URL alone is not a complete backend"
    );
    let (st, _) =
        put(json!({"FLICKDD_JELLYFIN_URL": null, "FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s.state.settings.lookup("FLICKDD_JELLYFIN_API_KEY"), None);

    // Module status never carries them either.
    let (_, mods) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert!(!mods.to_string().contains("secret"));
}

#[tokio::test]
async fn flickdd_can_be_configured_started_and_reloaded_from_the_api() {
    let s = server(&[]).await;
    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/start", None).await;
    assert_eq!(
        (st, m["state"].as_str()),
        (StatusCode::OK, Some("failed")),
        "no backend yet: {m}"
    );

    let (st, _) = call(&s, Method::PUT, "/admin/v1/settings/flickdd", Some(json!({"values": {"FLICKDD_JELLYFIN_URL": "http://jf:8096", "FLICKDD_JELLYFIN_API_KEY": "k"}}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/reload", None).await;
    assert_eq!(
        (m["state"].as_str(), m["pending_reload"].as_bool()),
        (Some("running"), Some(false)),
        "{m}"
    );
    assert!(s.state.dd().is_some());
}

#[tokio::test]
async fn the_server_scope_reloads_through_the_api() {
    let s = server(&[]).await;
    let (st, _) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/server",
        Some(json!({"values": {"FLICKSYNC_PUBLIC_URL": "https://flick.example.com/services"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, inv) = call(&s, Method::GET, "/admin/v1/invite", None).await;
    assert!(
        !inv["address"].as_str().unwrap().contains("services"),
        "not before the reload"
    );
    let (st, body) = call(&s, Method::POST, "/admin/v1/settings/server/reload", None).await;
    assert_eq!(
        (st, body["reloaded"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    let (_, inv) = call(&s, Method::GET, "/admin/v1/invite", None).await;
    assert_eq!(inv["address"], "https://flick.example.com/services");
    assert!(
        inv["url"]
            .as_str()
            .unwrap()
            .starts_with("flickserver://flick.example.com/services/")
    );
}

#[tokio::test]
async fn responses_are_never_cacheable() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let s = server(&[]).await;
    for (method, path, body) in [
        (Method::GET, "/admin/v1/modules", None),
        (Method::GET, "/admin/v1/settings/server", None),
        (
            Method::PUT,
            "/admin/v1/settings/flicksync",
            Some(r#"{"values":{"FLICKSYNC_MAX_ROOM_SIZE":0}}"#),
        ),
        (Method::POST, "/admin/v1/modules/flicksync/reload", None),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {ADMIN}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let resp = flicksync::app::build_router(s.state.clone())
            .oneshot(req.body(Body::from(body.unwrap_or(""))).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.headers()["cache-control"], "no-store", "{path}");
    }
}

#[tokio::test]
async fn concurrent_server_reloads_are_serialised() {
    let s = server(&[]).await;
    let (st, _) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/server",
        Some(json!({"values": {"FLICKSYNC_PUBLIC_URL": "https://last.example.com"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let state = s.state.clone();
            std::thread::spawn(move || state.reload_server().is_ok())
        })
        .collect();
    for t in threads {
        assert!(t.join().expect("no panic"));
    }
    let (_, inv) = call(&s, Method::GET, "/admin/v1/invite", None).await;
    assert_eq!(inv["address"], "https://last.example.com");
}

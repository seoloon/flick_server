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
    // (body, code, words the message must contain)
    for (body, code, words) in [
        (
            json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 0}}),
            "SETTINGS_INVALID",
            &["FLICKSYNC_MAX_ROOM_SIZE", "at least 1"][..],
        ),
        (
            json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": "ten"}}),
            "SETTINGS_INVALID",
            &["FLICKSYNC_MAX_ROOM_SIZE", "'ten'", "whole number"][..],
        ),
        (
            json!({"values": {"FLICKSYNC_HOST_LEAVE_POLICY": "explode"}}),
            "SETTINGS_INVALID",
            &["FLICKSYNC_HOST_LEAVE_POLICY", "transfer, close"][..],
        ),
        (
            json!({"values": {"FLICKSYNC_SYNC_DRIFT_SOFT": 50}}),
            "SETTINGS_INVALID",
            &[
                "FLICKSYNC_SYNC_DRIFT_IGNORE",
                "FLICKSYNC_SYNC_DRIFT_SOFT",
                "50",
            ][..],
        ),
        (
            json!({"values": {"NOPE": 1}}),
            "UNKNOWN_SETTING",
            &["NOPE", "GET /admin/v1/settings/flicksync"][..],
        ),
        (
            json!({"values": {"FLICKDD_MAX_PARALLEL": 1}}),
            "UNKNOWN_SETTING",
            &["FLICKDD_MAX_PARALLEL", "PUT /admin/v1/settings/flickdd"][..],
        ),
        (
            json!({"values": {"FLICKSYNC_PORT": 1}}),
            "UNKNOWN_SETTING",
            &["FLICKSYNC_PORT", "environment"][..],
        ),
        (
            json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": [1]}}),
            "INVALID_PAYLOAD",
            &["FLICKSYNC_MAX_ROOM_SIZE", "string, a number, a boolean"][..],
        ),
        (
            json!({"nothing": true}),
            "INVALID_PAYLOAD",
            &["{\"values\""][..],
        ),
    ] {
        let (st, v) = call(
            &s,
            Method::PUT,
            "/admin/v1/settings/flicksync",
            Some(body.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["error"]["code"], code, "{body}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        for w in words {
            assert!(message.contains(w), "{body}: '{w}' not in: {message}");
        }
    }
    assert_eq!(s.state.settings.revision(), before, "nothing was written");
}

#[tokio::test]
async fn a_body_that_is_not_json_is_explained() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let s = server(&[]).await;
    for (content_type, body, words) in [
        (Some("application/json"), "{not json", "not valid JSON"),
        (None, r#"{"values":{}}"#, "Content-Type"),
    ] {
        let mut req = Request::builder()
            .method(Method::PUT)
            .uri("/admin/v1/settings/flicksync")
            .header("authorization", format!("Bearer {ADMIN}"));
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        let resp = flicksync::app::build_router(s.state.clone())
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "INVALID_PAYLOAD", "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains(words),
            "{v}"
        );
    }
}

#[tokio::test]
async fn unknown_scopes_modules_and_actions_are_named_with_the_valid_ones() {
    let s = server(&[]).await;
    for (method, path, code, words) in [
        (
            Method::GET,
            "/admin/v1/settings/nope",
            "UNKNOWN_SCOPE",
            &["'nope'", "server, flicksync, flickdd"][..],
        ),
        (
            Method::PUT,
            "/admin/v1/settings/nope",
            "UNKNOWN_SCOPE",
            &["'nope'", "server, flicksync, flickdd"][..],
        ),
        (
            Method::POST,
            "/admin/v1/modules/nope/start",
            "UNKNOWN_MODULE",
            &["'nope'", "flicksync, flickdd"][..],
        ),
        (
            Method::POST,
            "/admin/v1/modules/flicksync/explode",
            "UNKNOWN_ACTION",
            &["'explode'", "start, stop, reload"][..],
        ),
    ] {
        let body = (method == Method::PUT).then(|| json!({"values": {}}));
        let (st, v) = call(&s, method, path, body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(v["error"]["code"], code, "{path}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        for w in words {
            assert!(message.contains(w), "{path}: '{w}' not in: {message}");
        }
    }
}

#[tokio::test]
async fn a_settings_write_failure_is_explained_without_the_path() {
    let s = server(&[]).await;
    let dir = s.state.settings.lookup("FLICKSYNC_DATA_DIR").unwrap();
    // A directory where the temporary file must go makes every save fail.
    std::fs::create_dir_all(std::path::Path::new(&dir).join("settings.json.tmp")).unwrap();
    let before = s.state.settings.revision();
    for (method, path, body) in [
        (
            Method::PUT,
            "/admin/v1/settings/flicksync",
            Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 7}})),
        ),
        (Method::POST, "/admin/v1/modules/flickdd/start", None),
    ] {
        let (st, v) = call(&s, method, path, body).await;
        assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{path}: {v}");
        assert_eq!(v["error"]["code"], "SETTINGS_WRITE_FAILED", "{path}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains("settings.json"), "{message}");
        assert!(message.contains("Nothing was changed"), "{message}");
        assert!(!message.contains(&dir), "no path: {message}");
    }
    assert_eq!(s.state.settings.revision(), before);
    assert!(s.state.dd().is_none(), "the module did not start");
}

#[tokio::test]
async fn a_server_reload_that_cannot_read_the_key_file_is_a_reload_failure() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    // No FLICKSYNC_AUTH_KEYS: the signing key is generated into the data directory.
    let dir = flicksync::settings::scratch_dir("reload-keys");
    let env_dir = dir.to_str().unwrap().to_owned();
    let env: flicksync::settings::Env = std::sync::Arc::new(move |k| match k {
        "FLICKSYNC_DATA_DIR" => Some(env_dir.clone()),
        "PANEL_PASSWORD" => Some(PASSWORD.to_owned()),
        _ => None,
    });
    let settings = flicksync::settings::Settings::open(&dir, env).unwrap();
    let state = flicksync::app::AppState::new(settings).unwrap();
    std::fs::write(dir.join(flicksync::invite::KEY_FILE), "").unwrap();

    let resp = flicksync::app::build_router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/admin/v1/settings/server/reload")
                .header("authorization", format!("Bearer {ADMIN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["code"], "RELOAD_FAILED", "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("auth_keys"), "{message}");
    assert!(message.contains("previous"), "{message}");
    assert!(
        !message.contains(dir.to_str().unwrap()),
        "no path: {message}"
    );
    assert!(state.server().auth.has_keys(), "the old keys stay in force");
    state.stop_all();
}

#[tokio::test]
async fn auth_failures_say_what_is_missing() {
    let s = server(&[]).await;
    let (st, v) = s.http(Method::GET, "/admin/v1/modules", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "UNAUTHENTICATED");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Authorization: Bearer"),
        "{v}"
    );
    let (st, v) = s
        .http(
            Method::GET,
            "/admin/v1/modules",
            Some("wrong-token-0123456789"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "UNAUTHENTICATED");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("PANEL_PASSWORD"), "{message}");
    assert!(!message.contains("wrong-token"), "{message}");
}

#[tokio::test]
async fn a_stopped_or_failed_module_is_explained_on_its_admin_routes() {
    let s = server(&[("FLICKSYNC_ENABLED", "false")]).await;
    let (st, v) = call(&s, Method::GET, "/admin/v1/rooms", None).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(v["error"]["code"], "MODULE_DISABLED");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("POST /admin/v1/modules/flicksync/start"),
        "{v}"
    );

    let (st, v) = call(&s, Method::GET, "/admin/v1/dd/overview", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "DOWNLOAD_NOT_FOUND");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("POST /admin/v1/modules/flickdd/start"),
        "{v}"
    );

    // Enabled without a backend: the reason is given, with what to do.
    let (_, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/start", None).await;
    let reason = m["message"].as_str().unwrap();
    assert!(reason.contains("FLICKDD_JELLYFIN_URL"), "{reason}");
    assert!(reason.contains("start the module again"), "{reason}");
    let (_, v) = call(&s, Method::GET, "/admin/v1/dd/overview", None).await;
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("failed to start"), "{message}");
    assert!(message.contains("FLICKDD_JELLYFIN_URL"), "{message}");
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
    let (st, v) = put(json!({"FLICKDD_JELLYFIN_API_KEY": "second-secret"})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        !v.to_string().contains("second-secret"),
        "the replace answer"
    );
    let (_, mods) = call(&s, Method::GET, "/admin/v1/modules", None).await;
    assert!(!mods.to_string().contains("second-secret"), "module status");
    assert!(
        !mods.to_string().contains("top-secret-jf-key"),
        "module status"
    );
    assert_eq!(
        s.state
            .settings
            .lookup("FLICKDD_JELLYFIN_API_KEY")
            .as_deref(),
        Some("second-secret")
    );
    let (st, v) = put(json!({"FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert!(!v.to_string().contains("second-secret"), "the refusal");
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "the URL alone is not a complete backend"
    );
    let (st, _) =
        put(json!({"FLICKDD_JELLYFIN_URL": null, "FLICKDD_JELLYFIN_API_KEY": null})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(s.state.settings.lookup("FLICKDD_JELLYFIN_API_KEY"), None);
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
async fn a_flickdd_enabled_without_backend_never_affects_the_rest() {
    let s = server(&[]).await;
    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/start", None).await;
    assert_eq!(
        (st, m["state"].as_str(), m["enabled"].as_bool()),
        (StatusCode::OK, Some("failed"), Some(true)),
        "{m}"
    );

    // (a) FlickSync still reloads.
    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flicksync/reload", None).await;
    assert_eq!(
        (st, m["state"].as_str()),
        (StatusCode::OK, Some("running")),
        "{m}"
    );

    // (b) The other scopes still save, the server runtime still reloads.
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/server",
        Some(json!({"values": {"FLICKSYNC_PUBLIC_URL": "https://flick.example.com"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flicksync",
        Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 7}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = call(&s, Method::POST, "/admin/v1/settings/server/reload", None).await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // (c) A restart on the same data directory (the FlickDD switch comes from the file only)
    // boots, FlickDD failed, FlickSync running.
    let dir = s.state.settings.lookup("FLICKSYNC_DATA_DIR").unwrap();
    let keys = s.state.settings.lookup("FLICKSYNC_AUTH_KEYS").unwrap();
    let env_dir = dir.clone();
    let env: flicksync::settings::Env = std::sync::Arc::new(move |k| match k {
        "FLICKSYNC_DATA_DIR" => Some(env_dir.clone()),
        "FLICKSYNC_AUTH_KEYS" => Some(keys.clone()),
        "FLICKSYNC_ENABLED" => Some("true".to_owned()),
        _ => None,
    });
    let settings = flicksync::settings::Settings::open(std::path::Path::new(&dir), env).unwrap();
    let again = flicksync::app::AppState::new(settings).expect("boots despite FlickDD");
    let dd = again.module_status(flicksync::modules::ModuleId::FlickDd);
    assert_eq!((dd.state, dd.enabled), ("failed", true));
    assert!(again.sync().is_ok(), "FlickSync is unaffected and runs");
    again.stop_all();
}

#[tokio::test]
async fn an_invalid_flickdd_environment_never_blocks_boot_or_other_scopes() {
    let s = server(&[("FLICKDD_ENABLED", "true"), ("FLICKDD_MAX_PARALLEL", "ten")]).await;
    let dd = s.state.module_status(flicksync::modules::ModuleId::FlickDd);
    assert_eq!(dd.state, "failed");
    assert!(dd.message.unwrap().contains("FLICKDD_MAX_PARALLEL"));
    assert!(s.state.sync().is_ok());
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flicksync",
        Some(json!({"values": {"FLICKSYNC_MAX_ROOM_SIZE": 7}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // A FlickDD save is still validated.
    let (st, _) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flickdd",
        Some(json!({"values": {"FLICKDD_MAX_GLOBAL": 50}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_unparseable_flickdd_enabled_is_refused_and_writes_nothing() {
    let s = server(&[]).await;
    let before = s.state.settings.revision();
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flickdd",
        Some(json!({"values": {"FLICKDD_ENABLED": "perhaps"}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("FLICKDD_ENABLED"),
        "{v}"
    );
    assert_eq!(s.state.settings.revision(), before, "nothing was written");
    assert_eq!(s.state.settings.lookup("FLICKDD_ENABLED"), None);
}

#[tokio::test]
async fn an_unparseable_flickdd_enabled_in_the_environment_fails_flickdd_by_name() {
    let s = server(&[("FLICKDD_ENABLED", "perhaps")]).await;
    let dd = s.state.module_status(flicksync::modules::ModuleId::FlickDd);
    assert_eq!((dd.state, dd.enabled), ("failed", true), "{dd:?}");
    assert!(dd.message.unwrap().contains("FLICKDD_ENABLED"));
    assert!(s.state.sync().is_ok(), "FlickSync is unaffected");
    // Stopping stores a valid switch, which then wins over the environment.
    let (st, m) = call(&s, Method::POST, "/admin/v1/modules/flickdd/stop", None).await;
    assert_eq!(
        (st, m["state"].as_str(), m["enabled"].as_bool()),
        (StatusCode::OK, Some("stopped"), Some(false)),
        "{m}"
    );
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
    for (method, path, body, expected) in [
        (Method::GET, "/admin/v1/modules", None, StatusCode::OK),
        (
            Method::GET,
            "/admin/v1/settings/server",
            None,
            StatusCode::OK,
        ),
        (
            Method::PUT,
            "/admin/v1/settings/flicksync",
            Some(r#"{"values":{"FLICKSYNC_MAX_ROOM_SIZE":0}}"#),
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/admin/v1/modules/flicksync/reload",
            None,
            StatusCode::OK,
        ),
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
        assert_eq!(resp.status(), expected, "{path}");
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

#[tokio::test]
async fn a_refused_secret_write_never_echoes_the_value() {
    const MARKER: &str = "MARKER-SECRET-0123456789-abcdefghij-xyz";
    let s = server(&[]).await;
    let revision = s.state.settings.revision();
    let keys_before = s.state.settings.lookup("FLICKSYNC_AUTH_KEYS");

    let bad_keys = format!("bad kid:srv:{MARKER}");
    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/server",
        Some(json!({"values": {"FLICKSYNC_AUTH_KEYS": bad_keys}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "SETTINGS_INVALID");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("FLICKSYNC_AUTH_KEYS"),
        "{v}"
    );
    assert!(!v.to_string().contains(MARKER), "server refusal body");
    assert_eq!(s.state.settings.lookup("FLICKSYNC_AUTH_KEYS"), keys_before);

    let (st, v) = call(
        &s,
        Method::PUT,
        "/admin/v1/settings/flickdd",
        Some(json!({"values": {"FLICKDD_PLEX_TOKEN": MARKER}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(!v.to_string().contains(MARKER), "flickdd refusal body");
    assert_eq!(s.state.settings.lookup("FLICKDD_PLEX_TOKEN"), None);

    assert_eq!(s.state.settings.revision(), revision, "nothing was written");
}

//! Shared harness: a real FlickSync server on an ephemeral port plus a tiny WebSocket client.
#![allow(dead_code)]

pub mod fake_media;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;

use flicksync::app::{AppState, build_router, spawn_sweeper};
use flicksync::auth::{Claims, mint_token};
use flicksync::settings::{Env, Settings};

pub const SECRET: &str = "integration-test-secret-0123456789abcdef";
pub const SERVER: &str = "flick-1";
pub const OTHER_SECRET: &str = "other-server-secret-0123456789abcdefgh";
pub const OTHER_SERVER: &str = "flick-2";

pub struct TestServer {
    pub addr: SocketAddr,
    pub state: AppState,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn token_for(user: &str, server: &str, kid: &str, secret: &str, perms: &[&str]) -> String {
    mint_token(
        kid,
        secret,
        &Claims {
            sub: user.into(),
            server_id: server.into(),
            aud: "flicksync".into(),
            exp: now_secs() + 600,
            iat: Some(now_secs()),
            name: Some(user.to_uppercase()),
            perms: perms.iter().map(|p| p.to_string()).collect(),
        },
    )
    .unwrap()
}

/// A token for `user` on the main Flick server with every permission.
pub fn token(user: &str) -> String {
    token_for(user, SERVER, "k1", SECRET, &["*"])
}

impl TestServer {
    pub async fn start(extra: &[(&str, &str)]) -> Self {
        let mut vars: HashMap<String, String> = HashMap::new();
        let mut set = |k: &str, v: &str| {
            vars.insert(k.to_string(), v.to_string());
        };
        set(
            "FLICKSYNC_AUTH_KEYS",
            &format!("k1:{SERVER}:{SECRET},k2:{OTHER_SERVER}:{OTHER_SECRET}"),
        );
        set("FLICKSYNC_SWEEP_INTERVAL_MS", "50");
        set("FLICKSYNC_RECONNECT_GRACE", "1");
        set("FLICKSYNC_ROOM_TIMEOUT", "1");
        set("FLICKSYNC_SYNC_HEARTBEAT", "1");
        set("FLICKSYNC_MSG_BURST", "1000");
        set("FLICKSYNC_MSG_RATE_PER_SEC", "1000");
        set("FLICKSYNC_ROOM_CREATE_PER_MINUTE", "1000");
        set("FLICKSYNC_WS_MAX_MESSAGE_BYTES", "4096");
        set("FLICKSYNC_CORS_ORIGINS", "https://app.example");
        set("FLICKSYNC_METRICS_ENABLED", "true");
        set("FLICKSYNC_ENABLED", "true");
        for (k, v) in extra {
            set(k, v);
        }
        let env: Env = Arc::new(move |k| vars.get(k).cloned());
        let dir = flicksync::settings::scratch_dir("server");
        let settings = Settings::open(&dir, env).unwrap();
        let state = AppState::new(settings).unwrap();
        std::mem::drop(spawn_sweeper(&state));
        let app = build_router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, state }
    }

    /// Issue a REST call straight into the router (shares state with the live server).
    pub async fn http(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = build_router(self.state.clone())
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    pub async fn create_room(&self, user: &str) -> String {
        let (status, body) = self
            .http(Method::POST, "/api/v1/rooms", Some(&token(user)), None)
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["room_id"].as_str().unwrap().to_string()
    }

    pub async fn join_http(&self, room: &str, user: &str) -> (StatusCode, Value) {
        self.http(
            Method::POST,
            &format!("/api/v1/rooms/{room}/join"),
            Some(&token(user)),
            None,
        )
        .await
    }

    pub async fn connect(&self, room: &str, user: &str) -> Ws {
        Ws::connect(self, room, &token(user)).await.unwrap()
    }
}

/// A fake Jellyfin/Plex serving a `size`-byte file and a server with FlickDD enabled, both
/// backends pointing at the fake, plus `extra` variables.
pub async fn start_dd(size: u64, extra: &[(&str, &str)]) -> (fake_media::FakeMedia, TestServer) {
    let fake = fake_media::FakeMedia::start(size).await;
    let env = fake.env();
    let mut vars: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    vars.extend_from_slice(extra);
    let server = TestServer::start(&vars).await;
    (fake, server)
}

pub struct Ws {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl Ws {
    pub async fn connect(
        server: &TestServer,
        room: &str,
        token: &str,
    ) -> Result<Ws, tungstenite::Error> {
        Self::connect_with(server, room, Some(token), &[]).await
    }

    pub async fn connect_with(
        server: &TestServer,
        room: &str,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> Result<Ws, tungstenite::Error> {
        let url = format!("ws://{}/api/v1/rooms/{room}/ws", server.addr);
        let mut req = url.into_client_request().unwrap();
        if let Some(t) = token {
            req.headers_mut()
                .insert("authorization", format!("Bearer {t}").parse().unwrap());
        }
        for (k, v) in headers {
            req.headers_mut().insert(
                tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        let (stream, _) = connect_async(req).await?;
        Ok(Ws { stream })
    }

    pub async fn send(&mut self, ty: &str, payload: Value) {
        let msg = json!({ "protocol_version": 1, "type": ty, "payload": payload });
        self.send_raw(&msg.to_string()).await;
    }

    pub async fn send_raw(&mut self, text: &str) {
        self.stream.send(Message::text(text)).await.unwrap();
    }

    pub async fn send_binary(&mut self, data: Vec<u8>) {
        self.stream.send(Message::binary(data)).await.unwrap();
    }

    /// Next text frame as JSON. Panics on timeout/close.
    pub async fn next_json(&mut self) -> Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(3), self.stream.next())
                .await
                .expect("timed out waiting for a message")
                .expect("stream ended")
                .expect("websocket error");
            match m {
                Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
                Message::Ping(_) | Message::Pong(_) => continue,
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    /// Next JSON message, or `None` if the connection closed / nothing arrived within `ms`.
    pub async fn try_next_json(&mut self, ms: u64) -> Option<Value> {
        loop {
            match tokio::time::timeout(Duration::from_millis(ms), self.stream.next()).await {
                Err(_) | Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).ok(),
                Ok(Some(Ok(Message::Close(_)))) => return None,
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// Skip messages until one of type `ty` arrives.
    pub async fn expect(&mut self, ty: &str) -> Value {
        for _ in 0..2000 {
            let v = self.next_json().await;
            if v["type"] == ty {
                assert_eq!(v["protocol_version"], 1);
                return v["payload"].clone();
            }
        }
        panic!("never received a '{ty}' message");
    }

    /// Like `expect` but also requires the payload to satisfy `pred`.
    pub async fn expect_where(&mut self, ty: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..2000 {
            let p = self.expect(ty).await;
            if pred(&p) {
                return p;
            }
        }
        panic!("no '{ty}' message matched the predicate");
    }

    /// Wait for the connection to end; returns the close code if the server sent one.
    pub async fn expect_closed(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), self.stream.next()).await {
                Err(_) => panic!("connection did not close"),
                Ok(None) | Ok(Some(Err(_))) => return None,
                Ok(Some(Ok(Message::Close(frame)))) => return frame.map(|f| u16::from(f.code)),
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// Assert that nothing (other than ping/pong) arrives for `ms`.
    pub async fn assert_silent(&mut self, ms: u64) {
        match tokio::time::timeout(Duration::from_millis(ms), async {
            loop {
                match self.stream.next().await {
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    other => return other,
                }
            }
        })
        .await
        {
            Err(_) => {}
            Ok(m) => panic!("expected silence, got {m:?}"),
        }
    }

    pub async fn close(mut self) {
        let _ = self.stream.close(None).await;
    }

    pub fn drop_abruptly(self) {
        drop(self);
    }
}

pub fn media(id: &str) -> Value {
    json!({
        "provider": "jellyfin",
        "server_id": "jf-server-1",
        "media_id": id,
        "media_type": "movie",
        "title": "Test Movie"
    })
}

/// Poll until `f` is true or a timeout elapses.
pub async fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition never became true: {what}");
}

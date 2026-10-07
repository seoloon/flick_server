//! FlickDD: media backends, the streaming pump and the client download routes.

mod common;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use common::eventually;
use common::fake_media::{FakeMedia, ITEM, expected};
use flicksync::dd::backend::{BackendError, BackendKind, Backends, ResolvedFile, valid_item_id};
use flicksync::dd::config::DdConfig;
use flicksync::dd::grants::{ActiveView, NewGrant, Progress};
use flicksync::dd::pump::{self, PumpParams};
use flicksync::dd::stats::Outcome;
use flicksync::dd::{DdState, now_ms};
use serde_json::{Value, json};
use tokio::sync::mpsc::Receiver;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// A `DdConfig` pointing both backends at `fake`, with `extra` overrides.
fn dd_config(fake: &FakeMedia, extra: &[(&str, &str)]) -> DdConfig {
    let mut vars: HashMap<String, String> = fake.env().into_iter().collect();
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    DdConfig::from_lookup(&move |k| vars.get(k).cloned()).unwrap()
}

async fn read_all(resp: reqwest::Response) -> Vec<u8> {
    resp.bytes().await.unwrap().to_vec()
}

// ---------------------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn resolve_reads_size_name_mime_and_a_stable_etag_on_both_backends() {
    let fake = FakeMedia::start(10 * KIB).await;
    let b = Backends::new(&dd_config(&fake, &[]));
    for kind in BackendKind::ALL {
        assert!(b.has(kind));
        let f = b.resolve(kind, ITEM).await.unwrap();
        assert_eq!(f.size, 10 * KIB, "{kind:?}");
        assert_eq!(f.filename, "Movie One (2020).mkv", "{kind:?}");
        assert_eq!(f.mime, "video/x-matroska", "{kind:?}");
        assert!(f.etag.starts_with('"') && f.etag.ends_with('"') && f.etag.len() == 34);
        let again = b.resolve(kind, ITEM).await.unwrap();
        assert_eq!(again.etag, f.etag, "{kind:?}: etag must be stable");
        fake.knobs.size.store(11 * KIB, Ordering::SeqCst);
        let changed = b.resolve(kind, ITEM).await.unwrap();
        assert_ne!(changed.etag, f.etag, "{kind:?}: etag must follow the file");
        fake.knobs.size.store(10 * KIB, Ordering::SeqCst);
    }
    let jf = b.resolve(BackendKind::Jellyfin, ITEM).await.unwrap();
    let plex = b.resolve(BackendKind::Plex, ITEM).await.unwrap();
    assert_eq!(jf.path, format!("/Items/{ITEM}/Download"));
    assert_eq!(plex.path, format!("/library/parts/1/{}/file.mkv", 10 * KIB));
    assert_ne!(jf.etag, plex.etag);
}

#[tokio::test]
async fn resolve_errors_unknown_item_bad_credentials_empty_file_unconfigured() {
    let fake = FakeMedia::start(10 * KIB).await;
    let b = Backends::new(&dd_config(&fake, &[]));
    for kind in BackendKind::ALL {
        assert!(matches!(
            b.resolve(kind, "nope").await,
            Err(BackendError::NotFound)
        ));
        // An invalid id never reaches the backend.
        assert!(matches!(
            b.resolve(kind, "../x").await,
            Err(BackendError::NotFound)
        ));
    }

    let bad = Backends::new(&dd_config(
        &fake,
        &[
            ("FLICKDD_JELLYFIN_API_KEY", "wrong"),
            ("FLICKDD_PLEX_TOKEN", "wrong"),
        ],
    ));
    for kind in BackendKind::ALL {
        assert!(matches!(
            bad.resolve(kind, ITEM).await,
            Err(BackendError::Unavailable(_))
        ));
    }

    fake.knobs.size.store(0, Ordering::SeqCst);
    for kind in BackendKind::ALL {
        // Resolved as is: `Grants::create` refuses the empty file with a 400.
        assert_eq!(b.resolve(kind, ITEM).await.unwrap().size, 0);
    }

    let mut vars: HashMap<String, String> = fake.env().into_iter().collect();
    vars.retain(|k, _| !k.starts_with("FLICKDD_PLEX"));
    let only_jf = Backends::new(&DdConfig::from_lookup(&move |k| vars.get(k).cloned()).unwrap());
    assert!(!only_jf.has(BackendKind::Plex));
    assert!(matches!(
        only_jf.resolve(BackendKind::Plex, ITEM).await,
        Err(BackendError::NotConfigured)
    ));
}

#[tokio::test]
async fn resolve_against_an_unreachable_backend_is_unavailable() {
    let fake = FakeMedia::start(10 * KIB).await;
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    }; // listener dropped: connection refused
    let b = Backends::new(&dd_config(&fake, &[("FLICKDD_JELLYFIN_URL", &dead)]));
    assert!(matches!(
        b.resolve(BackendKind::Jellyfin, ITEM).await,
        Err(BackendError::Unavailable(_))
    ));
}

#[tokio::test]
async fn open_serves_the_exact_range_and_detects_a_replaced_file() {
    let fake = FakeMedia::start(10 * KIB).await;
    let b = Backends::new(&dd_config(&fake, &[]));
    for kind in BackendKind::ALL {
        let f = b.resolve(kind, ITEM).await.unwrap();
        let r = b.open(kind, &f, 0, 9).await.unwrap();
        assert_eq!(read_all(r).await, expected(0, 10), "{kind:?}");
        assert_eq!(fake.last_range().as_deref(), Some("bytes=0-9"));
        let r = b.open(kind, &f, 5000, 10 * KIB - 1).await.unwrap();
        assert_eq!(
            read_all(r).await,
            expected(5000, 10 * KIB - 5000),
            "{kind:?}"
        );

        fake.knobs.size.store(12 * KIB, Ordering::SeqCst);
        assert!(
            matches!(
                b.open(kind, &f, 0, 9).await,
                Err(BackendError::SourceChanged)
            ),
            "{kind:?}"
        );
        // Shrunk below the requested start: the 416 carries the new total.
        fake.knobs.size.store(KIB, Ordering::SeqCst);
        assert!(
            matches!(
                b.open(kind, &f, 5000, 5009).await,
                Err(BackendError::SourceChanged)
            ),
            "{kind:?}"
        );
        fake.knobs.size.store(10 * KIB, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn open_gives_up_on_a_slow_first_byte() {
    let fake = FakeMedia::start(10 * KIB).await;
    let b = Backends::new(&dd_config(&fake, &[("FLICKDD_UPSTREAM_TIMEOUT", "1")]));
    let f = b.resolve(BackendKind::Jellyfin, ITEM).await.unwrap();
    fake.knobs.delay_first_byte_ms.store(1500, Ordering::SeqCst);
    let t = std::time::Instant::now();
    assert!(matches!(
        b.open(BackendKind::Jellyfin, &f, 0, 9).await,
        Err(BackendError::Unavailable(_))
    ));
    assert!(t.elapsed().as_millis() < 1800, "{:?}", t.elapsed());
}

#[tokio::test]
async fn open_on_a_vanished_item_is_not_found() {
    let fake = FakeMedia::start(10 * KIB).await;
    let b = Backends::new(&dd_config(&fake, &[]));
    let mut f = b.resolve(BackendKind::Jellyfin, ITEM).await.unwrap();
    f.path = "/Items/gone/Download".into();
    assert!(matches!(
        b.open(BackendKind::Jellyfin, &f, 0, 9).await,
        Err(BackendError::NotFound)
    ));
}

#[test]
fn item_ids_are_validated() {
    for bad in ["", "../x", "a/b", "a?b", "a b", "a%2F", &"x".repeat(65)] {
        assert!(!valid_item_id(bad), "{bad:?} must be rejected");
    }
    for good in ["movie1", "a", "AbC_09-z", &"x".repeat(64)] {
        assert!(valid_item_id(good), "{good:?} must be accepted");
    }
}

// ---------------------------------------------------------------------------------------
// Pump
// ---------------------------------------------------------------------------------------

/// A fake backend, a `DdState` pointing at it and one Jellyfin grant on `movie1`.
struct Setup {
    fake: FakeMedia,
    dd: Arc<DdState>,
    id: String,
    file: ResolvedFile,
}

impl Setup {
    async fn new(size: u64, extra: &[(&str, &str)]) -> Setup {
        let fake = FakeMedia::start(size).await;
        let dd = DdState::new(dd_config(&fake, extra));
        let file = dd
            .backends
            .resolve(BackendKind::Jellyfin, ITEM)
            .await
            .unwrap();
        let created = dd
            .grants
            .create(
                NewGrant {
                    user_id: "alice".into(),
                    user_name: "Alice".into(),
                    backend: BackendKind::Jellyfin,
                    item_id: ITEM.into(),
                    title: Some("Movie One".into()),
                    kind: Some("movie".into()),
                    file: file.clone(),
                },
                now_ms(),
            )
            .unwrap();
        Setup {
            fake,
            dd,
            id: created.id,
            file,
        }
    }

    /// Begin a segment, open `[start, end]` upstream and spawn the pump on it.
    async fn pump(&self, start: u64, end: u64) -> Receiver<io::Result<Bytes>> {
        let segment = self.dd.grants.begin_segment(&self.id, now_ms()).unwrap();
        let first = self
            .dd
            .backends
            .open(BackendKind::Jellyfin, &self.file, start, end)
            .await
            .unwrap();
        pump::spawn(PumpParams {
            dd: self.dd.clone(),
            grant_id: self.id.clone(),
            kind: BackendKind::Jellyfin,
            file: self.file.clone(),
            segment,
            start,
            end,
            first,
        })
    }

    fn view(&self) -> Option<ActiveView> {
        self.dd
            .grants
            .active_views(now_ms())
            .into_iter()
            .find(|v| v.download_id == self.id)
    }

    fn streaming(&self) -> bool {
        self.view().is_some_and(|v| v.streaming)
    }
}

/// Everything the body would carry: the bytes, and the errors (with their position).
struct Drained {
    bytes: Vec<u8>,
    errors: usize,
    error_was_last: bool,
}

/// Read `rx` to its end; panics if it does not end within `limit`.
async fn drain(rx: &mut Receiver<io::Result<Bytes>>, limit: Duration) -> Drained {
    let deadline = tokio::time::Instant::now() + limit;
    let mut d = Drained {
        bytes: Vec::new(),
        errors: 0,
        error_was_last: false,
    };
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => panic!("the body did not end within {limit:?}"),
            Ok(None) => return d,
            Ok(Some(Ok(b))) => {
                assert!(b.len() <= 64 * 1024, "piece of {} bytes", b.len());
                d.bytes.extend_from_slice(&b);
                d.error_was_last = false;
            }
            Ok(Some(Err(_))) => {
                d.errors += 1;
                d.error_was_last = true;
            }
        }
    }
}

#[tokio::test]
async fn pump_delivers_exact_bytes_and_respects_the_rate() {
    let s = Setup::new(3 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let t = Instant::now();
    let mut rx = s.pump(0, 3 * MIB - 1).await;
    let d = drain(&mut rx, Duration::from_secs(10)).await;
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(d.errors, 0);
    assert!(d.bytes == expected(0, 3 * MIB), "wrong bytes");
    // (3 MiB - 256 KiB of burst) at 1 MiB/s = 2.75 s.
    assert!((2.5..4.5).contains(&secs), "took {secs} s");
    // Full coverage completes the grant.
    eventually("grant completed", || s.dd.grants.count() == 0).await;
    let h = s.dd.stats.history();
    assert_eq!((h[0].outcome, h[0].covered), (Outcome::Completed, 3 * MIB));
    assert_eq!(s.dd.stats.snapshot(now_ms()).totals.bytes_served, 3 * MIB);
}

#[tokio::test]
async fn pump_retries_upstream_transparently() {
    let s = Setup::new(MIB, &[]).await;
    s.fake.knobs.cut_after.store(100_000, Ordering::SeqCst);
    let mut rx = s.pump(0, MIB - 1).await;
    let d = drain(&mut rx, Duration::from_secs(5)).await;
    assert_eq!(d.errors, 0);
    assert!(
        d.bytes == expected(0, MIB),
        "wrong bytes ({} received)",
        d.bytes.len()
    );
    assert_eq!(s.fake.requests(), 2);
    let retry = s.fake.last_range().unwrap();
    assert!(
        retry.starts_with("bytes=")
            && !retry.starts_with("bytes=0-")
            && retry.ends_with("-1048575"),
        "{retry}"
    );
    assert_eq!(s.dd.stats.snapshot(now_ms()).totals.upstream_errors, 1);
}

#[tokio::test]
async fn pump_gives_up_after_the_retry_budget_and_aborts_the_body() {
    let s = Setup::new(MIB, &[("FLICKDD_UPSTREAM_RETRIES", "0")]).await;
    s.fake.knobs.cut_after.store(100_000, Ordering::SeqCst);
    let mut rx = s.pump(0, MIB - 1).await;
    let d = drain(&mut rx, Duration::from_secs(5)).await;
    assert_eq!((d.errors, d.error_was_last), (1, true));
    assert!(d.bytes.len() <= 100_000);
    assert!(d.bytes == expected(0, d.bytes.len() as u64));
    assert_eq!(s.fake.requests(), 1);
    // The grant stays resumable; its segment is over and what was sent is accounted.
    eventually("segment ended", || !s.streaming()).await;
    assert_eq!(s.dd.grants.count(), 1);
    assert_eq!(s.view().unwrap().covered, d.bytes.len() as u64);
}

#[tokio::test]
async fn pump_stops_on_stall() {
    let s = Setup::new(32 * MIB, &[("FLICKDD_STALL_TIMEOUT", "1")]).await;
    let t = Instant::now();
    let rx = s.pump(0, 32 * MIB - 1).await; // never read
    assert!(s.streaming());
    eventually("segment ended", || !s.streaming()).await;
    let secs = t.elapsed().as_secs_f64();
    assert!((0.9..2.0).contains(&secs), "stall detected after {secs} s");
    // Only what was handed to the body is accounted: at most the 4 queued pieces.
    let v = s.view().expect("the grant stays resumable");
    assert!(v.covered > 0 && v.covered <= 4 * 64 * KIB, "{}", v.covered);
    // The upstream response was dropped: the fake sees its body go away.
    eventually("upstream released", || s.fake.open_bodies() == 0).await;
    drop(rx);
}

#[tokio::test]
async fn pump_stops_when_the_receiver_is_dropped() {
    let s = Setup::new(8 * MIB, &[]).await;
    let mut rx = s.pump(0, 8 * MIB - 1).await;
    assert!(rx.recv().await.unwrap().is_ok());
    drop(rx);
    eventually("segment ended", || !s.streaming()).await;
    assert_eq!(s.dd.grants.count(), 1, "the grant stays resumable");
    eventually("upstream released", || s.fake.open_bodies() == 0).await;
}

#[tokio::test]
async fn pump_stops_when_preempted() {
    let s = Setup::new(4 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let mut rx = s.pump(0, 4 * MIB - 1).await;
    assert!(rx.recv().await.unwrap().is_ok());
    let newer = s.dd.grants.begin_segment(&s.id, now_ms()).unwrap();
    let t = Instant::now();
    let d = drain(&mut rx, Duration::from_secs(2)).await;
    assert_eq!(d.errors, 0, "preemption ends the body quietly");
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert!(d.bytes.len() < 4 * MIB as usize);
    // The old pump's end must not clear the newer segment.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(s.streaming());
    eventually("upstream released", || s.fake.open_bodies() == 0).await;
    drop(newer);
}

#[tokio::test]
async fn pump_stops_when_an_admin_cancels() {
    let s = Setup::new(4 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let mut rx = s.pump(0, 4 * MIB - 1).await;
    assert!(rx.recv().await.unwrap().is_ok());
    assert!(s.dd.grants.cancel(&s.id, true, now_ms()));
    let d = drain(&mut rx, Duration::from_secs(1)).await;
    assert!(d.bytes.len() < 4 * MIB as usize);
    assert_eq!(s.dd.grants.count(), 0);
}

#[tokio::test]
async fn pump_keeps_streaming_when_the_grant_completes_underneath() {
    // The grant completes (another segment covered the rest) while this segment still has
    // bytes the client asked for: its cancel sender is dropped without a signal. The body
    // must still be delivered to the end, without further accounting.
    let s = Setup::new(MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let mut rx = s.pump(0, MIB - 1).await;
    let first = rx.recv().await.unwrap().unwrap();
    assert_eq!(
        s.dd.grants.record_progress(&s.id, 0, 0, MIB, now_ms()),
        Progress::Complete
    );
    assert_eq!(s.dd.grants.count(), 0);
    let mut d = drain(&mut rx, Duration::from_secs(3)).await;
    d.bytes.splice(0..0, first);
    assert_eq!(d.errors, 0);
    assert!(
        d.bytes == expected(0, MIB),
        "truncated: {} bytes",
        d.bytes.len()
    );
    assert_eq!(s.dd.stats.snapshot(now_ms()).totals.bytes_served, MIB);
}

#[tokio::test]
async fn pump_source_changed_mid_stream_invalidates_the_grant() {
    let s = Setup::new(MIB, &[]).await;
    s.fake.knobs.cut_after.store(100_000, Ordering::SeqCst);
    let mut rx = s.pump(0, MIB - 1).await;
    // The file is replaced while the (cut) first response is in flight.
    s.fake.knobs.size.store(MIB + 1000, Ordering::SeqCst);
    let d = drain(&mut rx, Duration::from_secs(5)).await;
    assert!(!d.bytes.is_empty() && d.bytes.len() <= 100_000);
    assert_eq!((d.errors, d.error_was_last), (1, true));
    assert_eq!(s.fake.requests(), 2);
    assert_eq!(s.dd.grants.count(), 0);
    let h = s.dd.stats.history();
    assert_eq!(h[0].outcome, Outcome::SourceChanged);
    assert_eq!(h[0].covered, d.bytes.len() as u64);
}

// ---------------------------------------------------------------------------------------
// App wiring: state, metrics, sweeper
// ---------------------------------------------------------------------------------------

async fn enabled_server(fake: &FakeMedia, extra: &[(&str, &str)]) -> common::TestServer {
    let mut vars: Vec<(&str, &str)> = vec![
        ("FLICKDD_ENABLED", "true"),
        ("FLICKDD_JELLYFIN_URL", &fake.url),
        ("FLICKDD_JELLYFIN_API_KEY", common::fake_media::JF_KEY),
    ];
    vars.extend_from_slice(extra);
    common::TestServer::start(&vars).await
}

async fn metrics_text(s: &common::TestServer) -> String {
    reqwest::get(format!("http://{}/metrics", s.addr))
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn dd_state_exists_only_when_enabled() {
    let off = common::TestServer::start(&[]).await;
    assert!(off.state.dd.is_none());
    let fake = FakeMedia::start(KIB).await;
    let on = enabled_server(&fake, &[]).await;
    assert!(on.state.dd.is_some());
}

#[tokio::test]
async fn metrics_include_flickdd_only_when_enabled() {
    let off = common::TestServer::start(&[]).await;
    let text = metrics_text(&off).await;
    assert!(!text.is_empty() && !text.contains("flickdd_"), "{text}");
    let fake = FakeMedia::start(KIB).await;
    let on = enabled_server(&fake, &[]).await;
    let text = metrics_text(&on).await;
    assert!(text.contains("flickdd_active_downloads 0"), "{text}");
}

#[tokio::test]
async fn the_sweeper_expires_idle_grants() {
    let fake = FakeMedia::start(10 * KIB).await;
    // 3 s of idle TTL: the grant is certainly still there for the metrics check below,
    // even on a slow machine; the sweeper runs every 50 ms (TestServer).
    let s = enabled_server(&fake, &[("FLICKDD_GRANT_TTL", "3")]).await;
    let dd = s.state.dd.clone().unwrap();
    let file = dd
        .backends
        .resolve(BackendKind::Jellyfin, ITEM)
        .await
        .unwrap();
    dd.grants
        .create(
            NewGrant {
                user_id: "alice".into(),
                user_name: "Alice".into(),
                backend: BackendKind::Jellyfin,
                item_id: ITEM.into(),
                title: None,
                kind: None,
                file,
            },
            now_ms(),
        )
        .unwrap();
    let created = Instant::now();
    assert_eq!(dd.grants.count(), 1);
    let metrics = metrics_text(&s).await;
    assert!(metrics.contains("flickdd_active_downloads 1"), "{metrics}");
    // Poll until swept, with a generous deadline (TTL + 7 s) rather than a fixed count.
    let deadline = created + Duration::from_secs(10);
    while dd.grants.count() != 0 {
        assert!(Instant::now() < deadline, "the grant was never swept");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let after = created.elapsed();
    assert!(
        after >= Duration::from_millis(2500),
        "swept too early: {after:?}"
    );
    assert_eq!(dd.stats.history()[0].outcome, Outcome::Expired);
}

// ---------------------------------------------------------------------------------------
// Client API (real sockets)
// ---------------------------------------------------------------------------------------

/// A fake backend, a server with FlickDD enabled on it and an HTTP client.
struct Api {
    fake: FakeMedia,
    server: common::TestServer,
    http: reqwest::Client,
}

/// A created grant.
struct Created {
    id: String,
    token: String,
    json: Value,
}

/// Status and JSON body (Null when not JSON) of a response.
async fn json_of(resp: reqwest::Response) -> (u16, Value) {
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
}

fn download_jwt(user: &str) -> String {
    common::token_for(
        user,
        common::SERVER,
        "k1",
        common::SECRET,
        &["downloads:create"],
    )
}

/// Read the body until it ends or fails: the bytes received, and whether it failed.
async fn read_until_end(mut resp: reqwest::Response) -> (Vec<u8>, bool) {
    let mut got = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), resp.chunk()).await {
            Err(_) => panic!("the body neither progressed nor ended"),
            Ok(Ok(Some(c))) => got.extend_from_slice(&c),
            Ok(Ok(None)) => return (got, false),
            Ok(Err(_)) => return (got, true),
        }
    }
}

impl Api {
    async fn start(size: u64, extra: &[(&str, &str)]) -> Api {
        let (fake, server) = common::start_dd(size, extra).await;
        Api {
            fake,
            server,
            http: reqwest::Client::new(),
        }
    }

    fn dd(&self) -> Arc<DdState> {
        self.server.state.dd.clone().unwrap()
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.server.addr)
    }

    async fn post_create(&self, jwt: &str, body: Value) -> (u16, Value) {
        let resp = self
            .http
            .post(self.url("/api/v1/downloads"))
            .bearer_auth(jwt)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        json_of(resp).await
    }

    /// Create a Jellyfin grant on the fake item as `user`; asserts 201.
    async fn create(&self, user: &str) -> Created {
        let body = json!({
            "backend": "jellyfin", "item_id": ITEM, "title": "  Movie One  ", "kind": "movie"
        });
        let (status, json) = self.post_create(&download_jwt(user), body).await;
        assert_eq!(status, 201, "{json}");
        Created {
            id: json["download_id"].as_str().unwrap().to_owned(),
            token: json["token"].as_str().unwrap().to_owned(),
            json,
        }
    }

    fn file(&self, g: &Created) -> reqwest::RequestBuilder {
        self.http
            .get(self.url(&format!("/api/v1/downloads/{}/file", g.id)))
            .bearer_auth(&g.token)
    }

    async fn status(&self, g: &Created) -> (u16, Value) {
        let resp = self
            .http
            .get(self.url(&format!("/api/v1/downloads/{}", g.id)))
            .bearer_auth(&g.token)
            .send()
            .await
            .unwrap();
        json_of(resp).await
    }
}

#[tokio::test]
async fn full_flow_create_download_verify_bytes() {
    let api = Api::start(MIB, &[]).await;
    let before = now_ms();
    let g = api.create("alice").await;
    let j = &g.json;
    assert_eq!(j["url"], format!("/api/v1/downloads/{}/file", g.id));
    assert_eq!(j["size"], MIB);
    assert_eq!(j["filename"], "Movie One (2020).mkv");
    assert_eq!(j["mime"], "video/x-matroska");
    let etag = j["etag"].as_str().unwrap().to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert_eq!(j["chunk_bytes"], 8 * MIB);
    assert_eq!(j["max_range_bytes"], 64 * MIB);
    assert_eq!(j["rate_limit_bps"], 10 * MIB);
    assert!(j["expires_at"].as_u64().unwrap() > before, "{j}");
    assert!(g.token.len() >= 40, "256-bit token");

    let (status, view) = api.status(&g).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["download_id"], g.id.as_str());
    assert_eq!(view["size"], MIB);
    assert_eq!(view["covered"], 0);
    assert_eq!(view["etag"], etag.as_str());
    assert!(view["expires_at"].as_u64().unwrap() > before);

    let resp = api.file(&g).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(header(&resp, "content-length"), Some(MIB.to_string()));
    assert_eq!(header(&resp, "accept-ranges").as_deref(), Some("bytes"));
    assert_eq!(header(&resp, "etag"), Some(etag));
    assert_eq!(
        header(&resp, "content-type").as_deref(),
        Some("video/x-matroska")
    );
    assert_eq!(header(&resp, "cache-control").as_deref(), Some("no-store"));
    assert!(header(&resp, "content-range").is_none());
    let cd = header(&resp, "content-disposition").unwrap();
    assert!(
        cd.starts_with("attachment; filename=\"Movie One (2020).mkv\"")
            && cd.contains("filename*=UTF-8''Movie%20One%20%282020%29.mkv"),
        "{cd}"
    );
    let body = read_all(resp).await;
    assert!(
        body == expected(0, MIB),
        "wrong bytes ({} received)",
        body.len()
    );

    let dd = api.dd();
    eventually("grant completed", || dd.grants.count() == 0).await;
    assert_eq!(api.status(&g).await.0, 404);
    let h = dd.stats.history();
    assert_eq!((h[0].outcome, h[0].covered), (Outcome::Completed, MIB));
    assert_eq!(h[0].title.as_deref(), Some("Movie One"));
    assert_eq!(h[0].kind.as_deref(), Some("movie"));
}

#[tokio::test]
async fn missing_permission_is_forbidden_and_disabled_is_404() {
    let api = Api::start(KIB, &[]).await;
    let rooms_only = common::token_for(
        "alice",
        common::SERVER,
        "k1",
        common::SECRET,
        &["rooms:create"],
    );
    let body = json!({ "backend": "jellyfin", "item_id": ITEM });
    let (status, json) = api.post_create(&rooms_only, body.clone()).await;
    assert_eq!((status, &json["error"]["code"]), (403, &json!("FORBIDDEN")));
    let resp = api
        .http
        .post(api.url("/api/v1/downloads"))
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 401);

    let off = common::TestServer::start(&[]).await;
    let http = reqwest::Client::new();
    let url = |p: &str| format!("http://{}{p}", off.addr);
    let jwt = common::token("alice");
    let requests = [
        http.post(url("/api/v1/downloads"))
            .bearer_auth(&jwt)
            .body(body.to_string()),
        http.get(url("/api/v1/downloads/abc")).bearer_auth("t"),
        http.get(url("/api/v1/downloads/abc/file")).bearer_auth("t"),
        http.delete(url("/api/v1/downloads/abc")).bearer_auth("t"),
    ];
    for rb in requests {
        let (status, json) = json_of(rb.send().await.unwrap()).await;
        assert_eq!(status, 404, "{json}");
        assert_eq!(json["error"]["code"], "DOWNLOAD_NOT_FOUND");
    }
}

#[tokio::test]
async fn ranged_resume_assembles_the_file() {
    let size = 5 * MIB;
    let api = Api::start(
        size,
        &[("FLICKDD_MAX_RANGE_MB", "1"), ("FLICKDD_CHUNK_MB", "1")],
    )
    .await;
    let g = api.create("alice").await;
    assert_eq!(g.json["max_range_bytes"], MIB);
    let mut file: Vec<u8> = Vec::new();
    let mut fragments = 0;
    let mut cut_once = false;
    while (file.len() as u64) < size {
        let off = file.len() as u64;
        let mut resp = api
            .file(&g)
            .header("range", format!("bytes={off}-"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 206, "fragment at {off}");
        let end = (off + MIB - 1).min(size - 1);
        assert_eq!(
            header(&resp, "content-range"),
            Some(format!("bytes {off}-{end}/{size}"))
        );
        assert_eq!(
            header(&resp, "content-length"),
            Some((end - off + 1).to_string())
        );
        fragments += 1;
        if fragments == 2 && !cut_once {
            // Drop the connection mid-fragment, keep what actually arrived.
            cut_once = true;
            let mut got = 0;
            while got < 300 * KIB {
                let c = resp.chunk().await.unwrap().unwrap();
                got += c.len() as u64;
                file.extend_from_slice(&c);
            }
            drop(resp);
            continue;
        }
        let body = read_all(resp).await;
        assert!(body.len() as u64 <= MIB);
        file.extend_from_slice(&body);
    }
    assert!(cut_once);
    assert!(file == expected(0, size), "wrong bytes");
    let dd = api.dd();
    eventually("grant completed", || dd.grants.count() == 0).await;
    let h = dd.stats.history();
    assert_eq!((h[0].outcome, h[0].covered), (Outcome::Completed, size));
    assert!(h[0].resumes >= 5, "{}", h[0].resumes);
}

#[tokio::test]
async fn if_range_with_a_stale_etag_returns_the_whole_file() {
    let api = Api::start(64 * KIB, &[]).await;
    let g = api.create("alice").await;
    let etag = g.json["etag"].as_str().unwrap().to_owned();
    // Matching validator: the range is honoured.
    let resp = api
        .file(&g)
        .header("range", "bytes=10-19")
        .header("if-range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(
        header(&resp, "content-range").as_deref(),
        Some("bytes 10-19/65536")
    );
    assert_eq!(read_all(resp).await, expected(10, 10));
    // Stale (or weak) validator: the whole file. Each completes its grant, so one grant each.
    for stale in ["\"stale\"", &format!("W/{etag}")] {
        let g = api.create("alice").await;
        let resp = api
            .file(&g)
            .header("range", "bytes=10-19")
            .header("if-range", stale)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "{stale}");
        assert!(header(&resp, "content-range").is_none());
        assert_eq!(header(&resp, "content-length").as_deref(), Some("65536"));
        assert!(read_all(resp).await == expected(0, 64 * KIB));
    }
}

#[tokio::test]
async fn eleventh_download_is_rejected() {
    let api = Api::start(KIB, &[]).await;
    let mut grants = Vec::new();
    for _ in 0..10 {
        grants.push(api.create("alice").await);
    }
    let body = json!({ "backend": "plex", "item_id": ITEM });
    let resolved = api.fake.metadata_requests();
    let (status, json) = api.post_create(&download_jwt("alice"), body.clone()).await;
    assert_eq!(status, 429, "{json}");
    assert_eq!(json["error"]["code"], "TOO_MANY_DOWNLOADS");
    assert_eq!(
        api.fake.metadata_requests(),
        resolved,
        "a refused creation never reaches the backend"
    );
    // Another user is unaffected.
    api.create("bob").await;
    // Cancelling one frees a slot.
    let g = &grants[3];
    let resp = api
        .http
        .delete(api.url(&format!("/api/v1/downloads/{}", g.id)))
        .bearer_auth(&g.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 204);
    assert_eq!(api.status(g).await.0, 404);
    let (status, json) = api.post_create(&download_jwt("alice"), body).await;
    assert_eq!(status, 201, "{json}");
    let h = api.dd().stats.history();
    assert_eq!(h[0].outcome, Outcome::Cancelled);
}

#[tokio::test]
async fn creation_is_rate_limited_per_user() {
    let api = Api::start(KIB, &[]).await;
    let jwt = download_jwt("alice");
    let body = json!({ "backend": "jellyfin", "item_id": ITEM });
    // A client looping on create: the first 10 get a slot, the rest of the burst is
    // refused by the slot pre-check, then the creation rate limit stops it.
    let mut codes = HashMap::new();
    for _ in 0..flicksync::dd::CREATE_PER_MINUTE {
        let (status, j) = api.post_create(&jwt, body.clone()).await;
        let code = j["error"]["code"].as_str().unwrap_or("CREATED").to_owned();
        *codes.entry((status, code)).or_insert(0) += 1;
    }
    assert_eq!(codes.get(&(201, "CREATED".into())), Some(&10), "{codes:?}");
    assert_eq!(
        codes.get(&(429, "TOO_MANY_DOWNLOADS".into())),
        Some(&20),
        "{codes:?}"
    );
    assert_eq!(
        api.fake.metadata_requests(),
        10,
        "one resolve per granted slot"
    );

    let resp = api
        .http
        .post(api.url("/api/v1/downloads"))
        .bearer_auth(&jwt)
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    let retry: u64 = header(&resp, "retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    let (_, j) = json_of(resp).await;
    assert_eq!(j["error"]["code"], "RATE_LIMITED");
    assert_eq!(api.fake.metadata_requests(), 10);
    let rejected = api.dd().stats.snapshot(now_ms()).rejected;
    assert_eq!(rejected.get("create"), Some(&1), "{rejected:?}");
    assert_eq!(rejected.get("slots"), Some(&20), "{rejected:?}");

    // Another user has its own budget.
    api.create("bob").await;
}

#[tokio::test]
async fn token_of_another_grant_is_refused() {
    let api = Api::start(KIB, &[]).await;
    let a = api.create("alice").await;
    let b = api.create("alice").await;
    let get = |path: String| api.http.get(api.url(&path));
    let status_path = format!("/api/v1/downloads/{}", a.id);
    let file_path = format!("/api/v1/downloads/{}/file", a.id);

    // Another grant's token, or garbage: not found.
    for t in [&b.token[..], "garbage"] {
        let resp = get(file_path.clone()).bearer_auth(t).send().await.unwrap();
        let (s, j) = json_of(resp).await;
        assert_eq!(
            (s, &j["error"]["code"]),
            (404, &json!("DOWNLOAD_NOT_FOUND"))
        );
        let resp = get(status_path.clone())
            .bearer_auth(t)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
    }
    // No token at all: unauthenticated.
    let (s, j) = json_of(get(file_path.clone()).send().await.unwrap()).await;
    assert_eq!((s, &j["error"]["code"]), (401, &json!("UNAUTHENTICATED")));

    // `?token=` works like the header...
    let with_query = |path: &str, t: &str| get(format!("{path}?token={t}"));
    let (s, j) = json_of(with_query(&status_path, &a.token).send().await.unwrap()).await;
    assert_eq!((s, &j["download_id"]), (200, &json!(a.id)));
    let resp = with_query(&status_path, &b.token).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    // ...but the header wins when both are present.
    let resp = with_query(&status_path, "garbage")
        .bearer_auth(&a.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = with_query(&status_path, &a.token)
        .bearer_auth("garbage")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let resp = with_query(&file_path, &a.token)
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(read_all(resp).await, expected(0, 100));
    let resp = api
        .http
        .delete(api.url(&format!("{status_path}?token={}", a.token)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 204);
    assert_eq!(api.status(&a).await.0, 404);
    assert_eq!(api.status(&b).await.0, 200, "the other grant is untouched");
}

#[tokio::test]
async fn hostile_ranges_and_item_ids() {
    let api = Api::start(1000, &[]).await;
    let g = api.create("alice").await;
    for range in ["bytes=5-1", "bytes=0-1,5-6", "bytes=1000-", "bytes=-0"] {
        let resp = api.file(&g).header("range", range).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 416, "{range}");
        assert_eq!(
            header(&resp, "content-range").as_deref(),
            Some("bytes */1000"),
            "{range}"
        );
        let (_, j) = json_of(resp).await;
        assert_eq!(j["error"]["code"], "RANGE_NOT_SATISFIABLE", "{range}");
    }
    let jwt = download_jwt("alice");
    for body in [
        json!({ "backend": "jellyfin", "item_id": "../x" }),
        json!({ "backend": "jellyfin", "item_id": "a/b" }),
        json!({ "backend": "jellyfin", "item_id": "x".repeat(65) }),
        json!({ "backend": "ftp", "item_id": ITEM }),
        json!({ "backend": "jellyfin", "item_id": ITEM, "kind": "album" }),
        json!({ "item_id": ITEM }),
        json!("not an object"),
    ] {
        let (status, j) = api.post_create(&jwt, body.clone()).await;
        assert_eq!(
            (status, &j["error"]["code"]),
            (400, &json!("INVALID_PAYLOAD")),
            "{body}"
        );
    }
    assert_eq!(
        api.fake.requests(),
        0,
        "no file request reached the backend"
    );
}

#[tokio::test]
async fn backend_errors_are_mapped_without_leaking_details() {
    let api = Api::start(KIB, &[]).await;
    let jwt = download_jwt("alice");
    let (status, j) = api
        .post_create(&jwt, json!({ "backend": "plex", "item_id": "nope" }))
        .await;
    assert_eq!(
        (status, &j["error"]["code"]),
        (404, &json!("DOWNLOAD_NOT_FOUND"))
    );
    // Only Jellyfin configured: Plex is 503.
    let only_jf = enabled_server(&api.fake, &[]).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/api/v1/downloads", only_jf.addr))
        .bearer_auth(&jwt)
        .body(json!({ "backend": "plex", "item_id": ITEM }).to_string())
        .send()
        .await
        .unwrap();
    let (status, j) = json_of(resp).await;
    assert_eq!(
        (status, &j["error"]["code"]),
        (503, &json!("BACKEND_UNAVAILABLE"))
    );
}

#[tokio::test]
async fn an_empty_file_is_refused_with_400() {
    let api = Api::start(KIB, &[]).await;
    api.fake.knobs.size.store(0, Ordering::SeqCst);
    let (status, j) = api
        .post_create(
            &download_jwt("alice"),
            json!({ "backend": "jellyfin", "item_id": ITEM }),
        )
        .await;
    assert_eq!(
        (status, &j["error"]["code"], &j["error"]["message"]),
        (400, &json!("INVALID_PAYLOAD"), &json!("empty file"))
    );
}

#[tokio::test]
async fn preemption() {
    let api = Api::start(8 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let g = api.create("alice").await;
    let first = api.file(&g).send().await.unwrap(); // never read
    assert_eq!(first.status().as_u16(), 200);
    let second = api
        .file(&g)
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(
        second.status().as_u16(),
        206,
        "a newer request preempts, never 409"
    );
    assert_eq!(read_all(second).await, expected(0, 100));
    let t = Instant::now();
    let (got, failed) = read_until_end(first).await;
    assert!(
        failed || (got.len() as u64) < 8 * MIB,
        "the first stream must end early ({} bytes)",
        got.len()
    );
    assert!((got.len() as u64) < 8 * MIB);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(got == expected(0, got.len() as u64));
    assert_eq!(api.dd().grants.count(), 1, "the grant stays");
}

#[tokio::test]
async fn shutdown_cuts_streams_and_refuses_new_work() {
    let api = Api::start(8 * MIB, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let g = api.create("alice").await;
    let resp = api.file(&g).send().await.unwrap(); // 8 s at 1 MiB/s
    assert_eq!(resp.status().as_u16(), 200);
    let dd = api.dd();
    assert_eq!(dd.shutdown(), 1);
    let t = Instant::now();
    let (got, _) = read_until_end(resp).await;
    assert!((got.len() as u64) < 8 * MIB, "{} bytes", got.len());
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    eventually("upstream released", || api.fake.open_bodies() == 0).await;

    let (status, j) = api
        .post_create(
            &download_jwt("bob"),
            json!({ "backend": "jellyfin", "item_id": ITEM }),
        )
        .await;
    assert_eq!((status, &j["error"]["code"]), (500, &json!("INTERNAL")));
    let resp = api
        .file(&g)
        .header("range", "bytes=0-9")
        .send()
        .await
        .unwrap();
    let (status, j) = json_of(resp).await;
    assert_eq!((status, &j["error"]["code"]), (500, &json!("INTERNAL")));
    assert_eq!(api.fake.requests(), 1, "no new upstream request");
}

#[tokio::test]
async fn source_changed_gives_409() {
    let api = Api::start(64 * KIB, &[]).await;
    let g = api.create("alice").await;
    api.fake.knobs.size.fetch_add(1, Ordering::SeqCst);
    let (status, j) = json_of(api.file(&g).send().await.unwrap()).await;
    assert_eq!(
        (status, &j["error"]["code"]),
        (409, &json!("SOURCE_CHANGED"))
    );
    assert_eq!(api.status(&g).await.0, 404);
    let h = api.dd().stats.history();
    assert_eq!(h[0].outcome, Outcome::SourceChanged);
}

#[tokio::test]
async fn rate_is_capped() {
    let size = 3 * MIB;
    let api = Api::start(size, &[("FLICKDD_RATE_MBPS", "1")]).await;
    let one = api.create("alice").await;
    let t = Instant::now();
    let body = read_all(api.file(&one).send().await.unwrap()).await;
    let secs = t.elapsed().as_secs_f64();
    assert!(body == expected(0, size), "wrong bytes");
    // (3 MiB - 256 KiB of burst) at 1 MiB/s = 2.75 s.
    assert!((2.5..4.5).contains(&secs), "took {secs} s");

    // Two grants at once: each is capped on its own (a shared cap would take twice as long).
    let (a, b) = (api.create("alice").await, api.create("alice").await);
    let t = Instant::now();
    let (ra, rb) = tokio::join!(
        async { read_all(api.file(&a).send().await.unwrap()).await },
        async { read_all(api.file(&b).send().await.unwrap()).await },
    );
    let secs = t.elapsed().as_secs_f64();
    assert!(ra.len() as u64 == size && rb.len() as u64 == size);
    assert!((2.5..4.5).contains(&secs), "took {secs} s");
}

#[tokio::test]
async fn request_rate_guard() {
    let api = Api::start(64 * KIB, &[("FLICKDD_MAX_REQUESTS_PER_MIN", "3")]).await;
    let g = api.create("alice").await;
    let ranged = || api.file(&g).header("range", "bytes=0-9");
    for _ in 0..3 {
        let resp = ranged().send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 206);
        assert_eq!(read_all(resp).await, expected(0, 10));
    }
    let resp = ranged().send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 429);
    let retry: u64 = header(&resp, "retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    let (_, j) = json_of(resp).await;
    assert_eq!(j["error"]["code"], "RATE_LIMITED");
    // The status route is not a stream request.
    assert_eq!(api.status(&g).await.0, 200);
}

#[tokio::test]
async fn no_range_request_with_cut_upstream_still_completes_or_truncates_cleanly() {
    let api = Api::start(MIB, &[("FLICKDD_UPSTREAM_RETRIES", "0")]).await;
    let g = api.create("alice").await;
    api.fake.knobs.cut_after.store(100_000, Ordering::SeqCst);
    let resp = api.file(&g).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(header(&resp, "content-length"), Some(MIB.to_string()));
    let (mut file, failed) = read_until_end(resp).await;
    assert!(failed, "a truncated body must be reported as an error");
    assert!(file.len() <= 100_000, "{}", file.len());
    assert!(file == expected(0, file.len() as u64));
    let off = file.len();
    let resp = api
        .file(&g)
        .header("range", format!("bytes={off}-"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    file.extend_from_slice(&read_all(resp).await);
    assert!(file == expected(0, MIB), "wrong bytes");
    let dd = api.dd();
    eventually("grant completed", || dd.grants.count() == 0).await;
}

#[tokio::test]
async fn head_does_not_preempt_or_open_upstream() {
    let api = Api::start(64 * KIB, &[]).await;
    let g = api.create("alice").await;
    let resp = api
        .http
        .head(api.url(&format!("/api/v1/downloads/{}/file", g.id)))
        .bearer_auth(&g.token)
        .header("range", "bytes=0-9")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(header(&resp, "content-length").as_deref(), Some("10"));
    assert_eq!(
        header(&resp, "content-range").as_deref(),
        Some("bytes 0-9/65536")
    );
    assert_eq!(api.fake.requests(), 0);
    let v = api.dd().grants.active_views(now_ms());
    assert_eq!((v[0].segments, v[0].streaming), (0, false));
}

//! FlickDD: media backends, the streaming pump and (Task 7) the download routes.

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
        assert!(matches!(
            b.resolve(kind, ITEM).await,
            Err(BackendError::Unavailable(_))
        ));
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
    assert!(t.elapsed().as_millis() < 1400, "{:?}", t.elapsed());
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
    assert!((2.5..3.6).contains(&secs), "took {secs} s");
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
    let s = enabled_server(&fake, &[("FLICKDD_GRANT_TTL", "1")]).await;
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
    assert_eq!(dd.grants.count(), 1);
    let metrics = metrics_text(&s).await;
    assert!(metrics.contains("flickdd_active_downloads 1"), "{metrics}");
    for _ in 0..60 {
        if dd.grants.count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the grant was never swept");
}

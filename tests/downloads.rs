//! FlickDD: media backends, the streaming pump and (Task 7) the download routes.

mod common;

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use common::fake_media::{FakeMedia, ITEM, expected};
use flicksync::dd::backend::{BackendError, BackendKind, Backends, valid_item_id};
use flicksync::dd::config::DdConfig;

const KIB: u64 = 1024;

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

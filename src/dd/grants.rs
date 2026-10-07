//! Download grant registry: tokens, slots, coverage, expiry, preemption.
//!
//! Time is always passed in (`now_ms`, wall clock): nothing in here reads a clock.
//! One mutex guards the whole registry; critical sections are short and never await.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::config::DdConfig;
use super::ranges::Coverage;
use super::stats::{FinishedDownload, Outcome, Stats};
use super::throttle::Throttle;
pub use super::types::{BackendKind, ResolvedFile};
use crate::api::auth::constant_time_eq;
use crate::errors::{Error, ErrorCode};

/// Throttle burst: 256 KiB, no full second of initial burst (spec section 4).
const BURST_BYTES: u64 = 256 * 1024;
const RATE_WINDOW_MS: u64 = 60_000;
/// Window of the "current speed" shown to admins.
const SPEED_WINDOW_MS: u64 = 5_000;
/// Bound of the speed samples kept per grant.
const MAX_SPEED_SAMPLES: usize = 256;

#[derive(Debug, Clone)]
pub struct NewGrant {
    pub user_id: String,
    pub user_name: String,
    pub backend: BackendKind,
    pub item_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub file: ResolvedFile,
}

#[derive(Clone)]
pub struct Created {
    pub id: String,
    pub token: String,
}

/// The token is a bearer secret: never printed.
impl std::fmt::Debug for Created {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Created")
            .field("id", &self.id)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Signal sent to the stream (pump) currently serving a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancel {
    No,
    /// Superseded by a newer request on the same grant, or ended by its owner / the server
    /// (user cancel, expiry, source change).
    Preempted,
    /// Cut by an administrator.
    Admin,
}

/// A stream segment handed to the pump.
#[derive(Debug)]
pub struct Segment {
    pub epoch: u64,
    pub cancel: watch::Receiver<Cancel>,
    pub throttle: Arc<Mutex<Throttle>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Continue,
    /// The grant is complete (or already gone): the stream has nothing more to account for.
    Complete,
    /// The grant reached its over-serve cap: it has been finished (`Outcome::Expired`).
    OverServed,
}

/// `begin_segment` error, with the `Retry-After` hint of the request rate guard.
#[derive(Debug, Clone)]
pub struct GrantError {
    pub error: Error,
    pub retry_after_secs: Option<u64>,
}

impl From<Error> for GrantError {
    fn from(error: Error) -> Self {
        Self {
            error,
            retry_after_secs: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GrantView {
    pub id: String,
    pub user_id: String,
    pub backend: BackendKind,
    pub size: u64,
    pub filename: String,
    pub mime: String,
    pub etag: String,
    pub covered: u64,
    pub expires_at: u64,
    pub file: ResolvedFile,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveView {
    pub download_id: String,
    pub user_id: String,
    pub user_name: String,
    pub backend: BackendKind,
    pub item_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub size: u64,
    pub covered: u64,
    pub served: u64,
    pub segments: u32,
    pub resumes: u32,
    pub started_at: u64,
    pub last_activity: u64,
    /// Bytes recorded over the last 5 s (or the grant's age when younger), per second.
    pub speed_bps: u64,
    pub streaming: bool,
}

struct Grant {
    token_hash: [u8; 32],
    user_id: String,
    user_name: String,
    backend: BackendKind,
    item_id: String,
    title: Option<String>,
    kind: Option<String>,
    file: ResolvedFile,
    coverage: Coverage,
    served: u64,
    segments: u32,
    resumes: u32,
    created_ms: u64,
    last_activity_ms: u64,
    req_times: VecDeque<u64>,
    active: Option<(u64, watch::Sender<Cancel>)>,
    next_epoch: u64,
    throttle: Arc<Mutex<Throttle>>,
    /// `(ms, bytes)` recorded by `record_progress`, last [`SPEED_WINDOW_MS`] only.
    speed: VecDeque<(u64, u64)>,
}

impl Grant {
    fn expired(&self, cfg: &DdConfig, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_activity_ms) > cfg.grant_ttl_ms
            || now_ms.saturating_sub(self.created_ms) > cfg.grant_max_age_ms
    }

    fn expires_at(&self, cfg: &DdConfig) -> u64 {
        self.last_activity_ms
            .saturating_add(cfg.grant_ttl_ms)
            .min(self.created_ms.saturating_add(cfg.grant_max_age_ms))
    }

    fn overserve_cap(&self, cfg: &DdConfig) -> u64 {
        self.file.size.saturating_mul(cfg.max_overserve).max(1)
    }

    fn touch(&mut self, now_ms: u64) {
        self.last_activity_ms = self.last_activity_ms.max(now_ms);
    }

    fn speed_bps(&self, now_ms: u64) -> u64 {
        let from = now_ms.saturating_sub(SPEED_WINDOW_MS);
        let bytes: u64 = self
            .speed
            .iter()
            .filter(|(t, _)| *t > from)
            .map(|(_, b)| b)
            .sum();
        let span = now_ms
            .saturating_sub(self.created_ms)
            .clamp(1000, SPEED_WINDOW_MS);
        bytes.saturating_mul(1000) / span
    }
}

#[derive(Default)]
struct Inner {
    grants: HashMap<String, Grant>,
    per_user: HashMap<String, usize>,
}

pub struct Grants {
    cfg: Arc<DdConfig>,
    stats: Arc<Stats>,
    inner: Mutex<Inner>,
}

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::rng().fill(&mut buf[..]);
    URL_SAFE_NO_PAD.encode(&buf)
}

fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn not_found() -> Error {
    Error::new(ErrorCode::DownloadNotFound, "download not found")
}

impl Grants {
    pub fn new(cfg: Arc<DdConfig>, stats: Arc<Stats>) -> Self {
        Self {
            cfg,
            stats,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `TooManyDownloads` (and the matching rejection counted) unless the server and
    /// `user_id` each have a free slot.
    fn admit(&self, inner: &Inner, user_id: &str) -> Result<(), Error> {
        if inner.grants.len() >= self.cfg.max_global {
            self.stats.record_rejected("global");
            return Err(Error::new(
                ErrorCode::TooManyDownloads,
                "the server has too many downloads in progress",
            ));
        }
        let held = inner.per_user.get(user_id).copied().unwrap_or(0);
        if held >= self.cfg.max_parallel {
            self.stats.record_rejected("slots");
            return Err(Error::new(
                ErrorCode::TooManyDownloads,
                format!("at most {} downloads in progress", self.cfg.max_parallel),
            ));
        }
        Ok(())
    }

    /// Pre-check of [`Grants::create`]'s slot limits, without taking a slot: lets the
    /// create route refuse before it asks the backend anything. `create` checks again.
    pub fn check_slots(&self, user_id: &str) -> Result<(), Error> {
        self.admit(&self.lock(), user_id)
    }

    /// Open a grant, taking one slot of the user and one of the server.
    /// An empty file is refused: a zero-byte grant has no valid range to serve.
    pub fn create(&self, new: NewGrant, now_ms: u64) -> Result<Created, Error> {
        if new.file.size == 0 {
            return Err(Error::new(ErrorCode::InvalidPayload, "empty file"));
        }
        let mut inner = self.lock();
        self.admit(&inner, &new.user_id)?;
        let id = loop {
            let id = random_b64(16);
            if !inner.grants.contains_key(&id) {
                break id;
            }
        };
        let token = random_b64(32);
        let grant = Grant {
            token_hash: hash_token(&token),
            user_id: new.user_id,
            user_name: new.user_name,
            backend: new.backend,
            item_id: new.item_id,
            title: new.title,
            kind: new.kind,
            file: new.file,
            coverage: Coverage::new(),
            served: 0,
            segments: 0,
            resumes: 0,
            created_ms: now_ms,
            last_activity_ms: now_ms,
            req_times: VecDeque::new(),
            active: None,
            next_epoch: 1,
            throttle: Arc::new(Mutex::new(Throttle::new(self.cfg.rate_bps, BURST_BYTES, 0))),
            speed: VecDeque::new(),
        };
        *inner.per_user.entry(grant.user_id.clone()).or_default() += 1;
        inner.grants.insert(id.clone(), grant);
        Ok(Created { id, token })
    }

    /// Check `token` against grant `id` and refresh its idle TTL. Unknown id, wrong token
    /// and expired grant all give the same `DownloadNotFound` (an expired grant is removed).
    pub fn authorize(&self, id: &str, token: &str, now_ms: u64) -> Result<GrantView, Error> {
        let mut inner = self.lock();
        let g = inner.grants.get_mut(id).ok_or_else(not_found)?;
        if !constant_time_eq(&g.token_hash, &hash_token(token)) {
            return Err(not_found());
        }
        if g.expired(&self.cfg, now_ms) {
            self.finish(&mut inner, id, Outcome::Expired, now_ms);
            return Err(not_found());
        }
        g.touch(now_ms);
        Ok(GrantView {
            id: id.to_owned(),
            user_id: g.user_id.clone(),
            backend: g.backend,
            size: g.file.size,
            filename: g.file.filename.clone(),
            mime: g.file.mime.clone(),
            etag: g.file.etag.clone(),
            covered: g.coverage.covered(),
            expires_at: g.expires_at(&self.cfg),
            file: g.file.clone(),
        })
    }

    /// Start a new stream on the grant: request rate guard, over-serve cap, then preempt the
    /// current stream (if any) and install the new one.
    pub fn begin_segment(&self, id: &str, now_ms: u64) -> Result<Segment, GrantError> {
        let mut inner = self.lock();
        let g = inner.grants.get_mut(id).ok_or_else(not_found)?;
        while g
            .req_times
            .front()
            .is_some_and(|&t| now_ms.saturating_sub(t) >= RATE_WINDOW_MS)
        {
            g.req_times.pop_front();
        }
        if g.req_times.len() >= self.cfg.max_requests_per_min as usize {
            let oldest = g.req_times.front().copied().unwrap_or(now_ms);
            let wait_ms = (oldest + RATE_WINDOW_MS).saturating_sub(now_ms);
            self.stats.record_rejected("rate");
            return Err(GrantError {
                error: Error::new(ErrorCode::RateLimited, "too many requests on this download"),
                retry_after_secs: Some(wait_ms.div_ceil(1000).max(1)),
            });
        }
        if g.served >= g.overserve_cap(&self.cfg) {
            self.stats.record_rejected("quota");
            // Spent: free the slot now rather than holding it until expiry.
            self.finish(&mut inner, id, Outcome::Expired, now_ms);
            return Err(Error::new(
                ErrorCode::QuotaExceeded,
                "this download has served its allowance; create a new one",
            )
            .into());
        }
        g.req_times.push_back(now_ms);
        g.touch(now_ms);
        if let Some((_, old)) = g.active.take() {
            old.send_replace(Cancel::Preempted);
        }
        let epoch = g.next_epoch;
        g.next_epoch += 1;
        let (tx, rx) = watch::channel(Cancel::No);
        g.active = Some((epoch, tx));
        g.segments = g.segments.saturating_add(1);
        if g.segments > 1 {
            g.resumes = g.resumes.saturating_add(1);
            self.stats.record_resume();
        }
        Ok(Segment {
            epoch,
            cancel: rx,
            throttle: g.throttle.clone(),
        })
    }

    /// Account `[start, end_excl)` served by segment `_epoch`. Bytes of a preempted segment
    /// still count (they were delivered). Completes the grant once its coverage is the file.
    pub fn record_progress(
        &self,
        id: &str,
        _epoch: u64,
        start: u64,
        end_excl: u64,
        now_ms: u64,
    ) -> Progress {
        let mut inner = self.lock();
        let Some(g) = inner.grants.get_mut(id) else {
            return Progress::Complete;
        };
        let len = end_excl.saturating_sub(start);
        g.served = g.served.saturating_add(len);
        g.coverage.add(start, end_excl);
        g.touch(now_ms);
        let from = now_ms.saturating_sub(SPEED_WINDOW_MS);
        while g.speed.front().is_some_and(|&(t, _)| t <= from) || g.speed.len() >= MAX_SPEED_SAMPLES
        {
            g.speed.pop_front();
        }
        g.speed.push_back((now_ms, len));
        self.stats.record_bytes(g.backend, len, now_ms);
        if g.coverage.is_complete(g.file.size) {
            self.finish(&mut inner, id, Outcome::Completed, now_ms);
            return Progress::Complete;
        }
        if g.served >= g.overserve_cap(&self.cfg) {
            // Spent: stop its stream and free the slot now rather than holding it until expiry.
            self.finish(&mut inner, id, Outcome::Expired, now_ms);
            return Progress::OverServed;
        }
        Progress::Continue
    }

    /// The stream of segment `epoch` ended: clear it unless a newer one replaced it.
    pub fn end_segment(&self, id: &str, epoch: u64) {
        let mut inner = self.lock();
        if let Some(g) = inner.grants.get_mut(id)
            && g.active.as_ref().is_some_and(|(e, _)| *e == epoch)
        {
            g.active = None;
        }
    }

    /// Cancel a grant (frees its slot) and stop its stream. False when unknown.
    pub fn cancel(&self, id: &str, by_admin: bool, now_ms: u64) -> bool {
        let mut inner = self.lock();
        let Some(g) = inner.grants.get_mut(id) else {
            return false;
        };
        if let Some((_, tx)) = g.active.take() {
            tx.send_replace(if by_admin {
                Cancel::Admin
            } else {
                Cancel::Preempted
            });
        }
        self.finish(&mut inner, id, Outcome::Cancelled, now_ms);
        true
    }

    /// The file changed on the backend: the grant is invalid, the client must start over.
    pub fn fail_source_changed(&self, id: &str, now_ms: u64) {
        let mut inner = self.lock();
        self.finish(&mut inner, id, Outcome::SourceChanged, now_ms);
    }

    /// Expire grants idle for longer than the TTL or older than the max age.
    pub fn sweep(&self, now_ms: u64) -> usize {
        let mut inner = self.lock();
        let ids: Vec<String> = inner
            .grants
            .iter()
            .filter(|(_, g)| g.expired(&self.cfg, now_ms))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.finish(&mut inner, id, Outcome::Expired, now_ms);
        }
        ids.len()
    }

    /// Live grants for the admin view, oldest first.
    pub fn active_views(&self, now_ms: u64) -> Vec<ActiveView> {
        let inner = self.lock();
        let mut v: Vec<ActiveView> = inner
            .grants
            .iter()
            .map(|(id, g)| ActiveView {
                download_id: id.clone(),
                user_id: g.user_id.clone(),
                user_name: g.user_name.clone(),
                backend: g.backend,
                item_id: g.item_id.clone(),
                title: g.title.clone(),
                kind: g.kind.clone(),
                size: g.file.size,
                covered: g.coverage.covered(),
                served: g.served,
                segments: g.segments,
                resumes: g.resumes,
                started_at: g.created_ms,
                last_activity: g.last_activity_ms,
                speed_bps: g.speed_bps(now_ms),
                streaming: g.active.is_some(),
            })
            .collect();
        v.sort_by(|a, b| (a.started_at, &a.download_id).cmp(&(b.started_at, &b.download_id)));
        v
    }

    pub fn count(&self) -> usize {
        self.lock().grants.len()
    }

    /// Remove grant `id`, free its slot, stop its stream (except on completion, where the
    /// stream ends by itself) and record it in the analytics.
    fn finish(&self, inner: &mut Inner, id: &str, outcome: Outcome, now_ms: u64) {
        let Some(mut g) = inner.grants.remove(id) else {
            return;
        };
        if let Some(n) = inner.per_user.get_mut(&g.user_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                inner.per_user.remove(&g.user_id);
            }
        }
        if outcome != Outcome::Completed
            && let Some((_, tx)) = g.active.take()
        {
            tx.send_replace(Cancel::Preempted);
        }
        self.stats.record_finished(
            FinishedDownload {
                download_id: id.to_owned(),
                user_id: g.user_id,
                user_name: g.user_name,
                backend: g.backend,
                item_id: g.item_id,
                title: g.title,
                kind: g.kind,
                size: g.file.size,
                covered: g.coverage.covered(),
                served: g.served,
                segments: g.segments,
                resumes: g.resumes,
                started_at: g.created_ms,
                finished_at: now_ms,
                outcome,
            },
            now_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dd::stats::Outcome;

    fn file() -> ResolvedFile {
        ResolvedFile {
            size: 100,
            filename: "f.mkv".into(),
            mime: "video/x-matroska".into(),
            etag: "\"e\"".into(),
            path: "/p".into(),
        }
    }

    fn grants_with(f: impl FnOnce(&mut DdConfig)) -> (Grants, Arc<Stats>) {
        let mut cfg = DdConfig::from_lookup(&|_| None).unwrap();
        f(&mut cfg);
        let stats = Arc::new(Stats::new());
        (Grants::new(Arc::new(cfg), stats.clone()), stats)
    }

    fn grants(max_parallel: usize, max_global: usize) -> (Grants, Arc<Stats>) {
        grants_with(|c| {
            c.max_parallel = max_parallel;
            c.max_global = max_global;
        })
    }

    fn new(user: &str, item: &str) -> NewGrant {
        NewGrant {
            user_id: user.to_owned(),
            user_name: user.to_uppercase(),
            backend: BackendKind::Jellyfin,
            item_id: item.to_owned(),
            title: Some(format!("Title {item}")),
            kind: Some("movie".into()),
            file: file(),
        }
    }

    #[test]
    fn slots_are_per_user_and_global() {
        let (g, _s) = grants(2, 3);
        g.create(new("alice", "a1"), 0).unwrap();
        g.create(new("alice", "a2"), 0).unwrap();
        assert_eq!(
            g.create(new("alice", "a3"), 0).unwrap_err().code,
            ErrorCode::TooManyDownloads
        );
        g.create(new("bob", "b1"), 0).unwrap();
        assert_eq!(
            g.create(new("carol", "c1"), 0).unwrap_err().code,
            ErrorCode::TooManyDownloads
        ); // global 3
    }

    #[test]
    fn slot_pre_check_matches_create_and_takes_nothing() {
        let (g, stats) = grants(2, 3);
        g.check_slots("alice").unwrap();
        g.check_slots("alice").unwrap(); // a check is not a reservation
        g.create(new("alice", "a1"), 0).unwrap();
        g.create(new("alice", "a2"), 0).unwrap();
        assert_eq!(
            g.check_slots("alice").unwrap_err().code,
            ErrorCode::TooManyDownloads
        );
        g.check_slots("bob").unwrap();
        g.create(new("bob", "b1"), 0).unwrap();
        assert_eq!(
            g.check_slots("carol").unwrap_err().code,
            ErrorCode::TooManyDownloads
        ); // global 3
        assert_eq!(g.count(), 3);
        let rejected = stats.snapshot(0).rejected;
        assert_eq!(
            (rejected.get("slots"), rejected.get("global")),
            (Some(&1), Some(&1))
        );
    }

    #[test]
    fn an_empty_file_is_refused_without_taking_a_slot() {
        let (g, _s) = grants(1, 1);
        let mut n = new("alice", "a1");
        n.file.size = 0;
        assert_eq!(g.create(n, 0).unwrap_err().code, ErrorCode::InvalidPayload);
        assert_eq!(g.count(), 0);
        g.create(new("alice", "a2"), 0).unwrap();
    }

    #[test]
    fn token_is_checked_and_bound_to_its_grant() {
        let (g, _s) = grants(2, 3);
        let a = g.create(new("alice", "a1"), 0).unwrap();
        let b = g.create(new("alice", "a2"), 0).unwrap();
        assert!(g.authorize(&a.id, &a.token, 1).is_ok());
        for bad in [&b.token[..], "", "x"] {
            assert_eq!(
                g.authorize(&a.id, bad, 1).unwrap_err().code,
                ErrorCode::DownloadNotFound
            );
        }
        assert_eq!(
            g.authorize("nope", &a.token, 1).unwrap_err().code,
            ErrorCode::DownloadNotFound
        );
    }

    #[test]
    fn a_new_segment_preempts_the_old_one() {
        let (g, _s) = grants(2, 3);
        let a = g.create(new("alice", "a1"), 0).unwrap();
        let s1 = g.begin_segment(&a.id, 1).unwrap();
        let s2 = g.begin_segment(&a.id, 2).unwrap();
        assert_eq!(*s1.cancel.borrow(), Cancel::Preempted);
        assert_eq!(*s2.cancel.borrow(), Cancel::No);
        assert!(s2.epoch > s1.epoch);
        // The old segment's late bookkeeping is ignored once preempted? No: its bytes still count.
        assert!(matches!(
            g.record_progress(&a.id, s1.epoch, 0, 10, 3),
            Progress::Continue
        ));
        g.end_segment(&a.id, s1.epoch); // must NOT clear the new segment
        assert!(g.begin_segment(&a.id, 4).is_ok());
    }

    #[test]
    fn completion_frees_the_slot_and_overlaps_are_not_double_counted() {
        let (g, stats) = grants(1, 1); // file size 100
        let a = g.create(new("alice", "a1"), 0).unwrap();
        let s = g.begin_segment(&a.id, 1).unwrap();
        assert!(matches!(
            g.record_progress(&a.id, s.epoch, 0, 60, 2),
            Progress::Continue
        ));
        assert!(matches!(
            g.record_progress(&a.id, s.epoch, 40, 90, 3),
            Progress::Continue
        ));
        assert!(matches!(
            g.record_progress(&a.id, s.epoch, 90, 100, 4),
            Progress::Complete
        ));
        assert_eq!(g.count(), 0);
        let h = stats.history();
        assert_eq!(
            (h[0].outcome, h[0].covered, h[0].served),
            (Outcome::Completed, 100, 120)
        );
        g.create(new("alice", "a2"), 5).unwrap(); // slot is free again
    }

    #[test]
    fn request_rate_guard_gives_a_retry_hint() {
        let (g, _s) = grants_with(|c| c.max_requests_per_min = 3);
        let a = g.create(new("alice", "a1"), 0).unwrap();
        for t in 0..3 {
            g.begin_segment(&a.id, t * 100).unwrap();
        }
        let e = g.begin_segment(&a.id, 400).unwrap_err();
        assert_eq!(e.error.code, ErrorCode::RateLimited);
        assert!(e.retry_after_secs.unwrap() >= 1);
        assert!(g.begin_segment(&a.id, 61_000).is_ok()); // window slid
    }

    #[test]
    fn over_serving_is_capped_and_finishes_the_grant() {
        let (g, stats) = grants_with(|c| {
            c.max_overserve = 2; // size 100 => 200 bytes max
            c.max_parallel = 1;
        });
        let a = g.create(new("alice", "a1"), 0).unwrap();
        let s = g.begin_segment(&a.id, 1).unwrap();
        g.record_progress(&a.id, s.epoch, 0, 50, 2);
        g.record_progress(&a.id, s.epoch, 0, 50, 2);
        g.record_progress(&a.id, s.epoch, 0, 50, 2);
        assert!(matches!(
            g.record_progress(&a.id, s.epoch, 0, 50, 2),
            Progress::OverServed
        ));
        // The grant is over: it no longer holds the user's slot until expiry.
        assert_eq!(g.count(), 0);
        assert_eq!(stats.history()[0].outcome, Outcome::Expired);
        assert_eq!(stats.history()[0].served, 200);
        assert_eq!(*s.cancel.borrow(), Cancel::Preempted);
        assert_eq!(
            g.begin_segment(&a.id, 3).unwrap_err().error.code,
            ErrorCode::DownloadNotFound
        );
        g.create(new("alice", "a2"), 4).unwrap(); // slot is free again
    }

    #[test]
    fn a_quota_refusal_finishes_the_grant() {
        let (g, stats) = grants_with(|c| {
            c.max_overserve = 2;
            c.max_parallel = 1;
        });
        let a = g.create(new("alice", "a1"), 0).unwrap();
        // Defensive path: normally `record_progress` finishes the grant first (OverServed).
        g.inner
            .lock()
            .unwrap()
            .grants
            .get_mut(&a.id)
            .unwrap()
            .served = 200;
        assert_eq!(
            g.begin_segment(&a.id, 1).unwrap_err().error.code,
            ErrorCode::QuotaExceeded
        );
        assert_eq!(g.count(), 0);
        assert_eq!(stats.history()[0].outcome, Outcome::Expired);
        assert!(g.inner.lock().unwrap().per_user.is_empty());
        g.create(new("alice", "a2"), 2).unwrap(); // slot is free again
    }

    #[test]
    fn created_debug_redacts_the_token() {
        let (g, _s) = grants(2, 3);
        let c = g.create(new("alice", "a1"), 0).unwrap();
        let dbg = format!("{c:?}");
        assert!(dbg.contains(&c.id), "{dbg}");
        assert!(!dbg.contains(&c.token), "{dbg}");
    }

    #[test]
    fn sweep_expires_idle_and_old_grants_and_cancel_signals_the_stream() {
        let (g, stats) = grants_with(|c| {
            c.grant_ttl_ms = 1000;
            c.grant_max_age_ms = 5000;
        });
        let a = g.create(new("alice", "a1"), 0).unwrap();
        assert_eq!(g.sweep(500), 0);
        assert_eq!(g.sweep(1500), 1);
        assert_eq!(stats.history()[0].outcome, Outcome::Expired);
        let b = g.create(new("alice", "a2"), 2000).unwrap();
        for t in [2900, 3800, 4700, 5600] {
            g.authorize(&b.id, &b.token, t).unwrap();
        } // activity keeps it alive...
        assert_eq!(g.sweep(7100), 1); // ...until the absolute max age (5 s)
        let c = g.create(new("alice", "a3"), 8000).unwrap();
        let s = g.begin_segment(&c.id, 8001).unwrap();
        assert!(g.cancel(&c.id, true, 8002));
        assert_eq!(*s.cancel.borrow(), Cancel::Admin);
        assert!(!g.cancel(&c.id, true, 8003)); // already gone
        let _ = a;
    }

    #[test]
    fn every_way_out_releases_the_user_slot() {
        let (g, stats) = grants_with(|c| {
            c.max_parallel = 1;
            c.grant_ttl_ms = 1000;
        });
        // cancel (by the user)
        let a = g.create(new("alice", "a1"), 0).unwrap();
        assert!(g.cancel(&a.id, false, 1));
        // expire (sweep)
        let b = g.create(new("alice", "a2"), 10).unwrap();
        assert_eq!(g.sweep(2000), 1);
        // complete
        let c = g.create(new("alice", "a3"), 2000).unwrap();
        let s = g.begin_segment(&c.id, 2001).unwrap();
        assert_eq!(
            g.record_progress(&c.id, s.epoch, 0, 100, 2002),
            Progress::Complete
        );
        // source changed
        let d = g.create(new("alice", "a4"), 2003).unwrap();
        g.fail_source_changed(&d.id, 2004);
        // Still exactly one slot free: one more succeeds, the next is refused.
        g.create(new("alice", "a5"), 2005).unwrap();
        assert_eq!(
            g.create(new("alice", "a6"), 2005).unwrap_err().code,
            ErrorCode::TooManyDownloads
        );
        assert_eq!(g.inner.lock().unwrap().per_user.get("alice"), Some(&1));
        let outcomes: Vec<Outcome> = stats.history().iter().map(|f| f.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Outcome::SourceChanged,
                Outcome::Completed,
                Outcome::Expired,
                Outcome::Cancelled
            ]
        );
        let _ = b;
    }

    #[test]
    fn authorize_on_an_expired_unswept_grant_fails_and_frees_the_slot() {
        let (g, stats) = grants_with(|c| {
            c.max_parallel = 1;
            c.grant_ttl_ms = 1000;
        });
        let a = g.create(new("alice", "a1"), 0).unwrap();
        assert_eq!(
            g.authorize(&a.id, &a.token, 1001).unwrap_err().code,
            ErrorCode::DownloadNotFound
        );
        assert_eq!(g.count(), 0);
        assert_eq!(stats.history()[0].outcome, Outcome::Expired);
        assert!(g.inner.lock().unwrap().per_user.is_empty());
        g.create(new("alice", "a2"), 1002).unwrap(); // slot is free again
    }
}

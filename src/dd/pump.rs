//! The streaming pump: upstream bytes -> per-grant throttle -> bounded channel -> HTTP body.
//!
//! One task per segment. Back-pressure is end to end: the channel holds at most
//! [`CHANNEL`] pieces of at most [`PIECE`] bytes, and upstream is only read when a piece has
//! been handed over, so a slow client slows the upstream read (TCP does the rest).
//!
//! The segment ends when its range is delivered, when it is preempted or cancelled (quietly),
//! when the client goes away or stops reading for `stall_timeout`, when the grant is
//! over-served, or when upstream fails beyond the retry budget or the source file changed
//! (an `Err` item, which aborts the body so the client sees a truncated response). Whatever
//! the way out, delivered bytes are accounted and the segment is released.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::sync::{mpsc, watch};

use super::backend::{BackendError, BackendKind, ResolvedFile};
use super::grants::{Cancel, Progress, Segment};
use super::throttle::Throttle;
use super::{DdState, now_ms};

/// Largest piece handed to the body (one throttle reservation).
const PIECE: usize = 64 * 1024;
/// Progress is reported to the grant registry every `FLUSH` bytes (and at the end).
const FLUSH: u64 = 1024 * 1024;
/// Pieces buffered between the pump and the body.
const CHANNEL: usize = 4;
/// Upstream retry backoff: 250 ms, then 1 s for every further attempt.
const BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

pub struct PumpParams {
    pub dd: Arc<DdState>,
    pub grant_id: String,
    pub kind: BackendKind,
    pub file: ResolvedFile,
    pub segment: Segment,
    pub start: u64,
    /// Inclusive.
    pub end: u64,
    /// The already opened (and verified) upstream response for `[start, end]`.
    pub first: reqwest::Response,
}

type Body = io::Result<Bytes>;
type Upstream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// Spawns the pump and returns the receiving half to build the body from.
pub fn spawn(p: PumpParams) -> mpsc::Receiver<Body> {
    let (tx, rx) = mpsc::channel(CHANNEL);
    tokio::spawn(run(p, tx));
    rx
}

async fn run(p: PumpParams, tx: mpsc::Sender<Body>) {
    // Declared first, dropped last: the segment is released after the final accounting.
    let _guard = EndGuard {
        dd: p.dd.clone(),
        id: p.grant_id.clone(),
        epoch: p.segment.epoch,
    };
    let stall = Duration::from_millis(p.dd.cfg.stall_timeout_ms);
    let idle = Duration::from_millis(p.dd.cfg.upstream_timeout_ms);
    let retries = p.dd.cfg.upstream_retries;
    let mut pump = Pump {
        dd: p.dd,
        id: p.grant_id,
        kind: p.kind,
        file: p.file,
        epoch: p.segment.epoch,
        end: p.end,
        tx,
        cancel: CancelWatch {
            rx: p.segment.cancel,
            live: true,
        },
        throttle: p.segment.throttle,
        stall,
        idle,
        retries,
        attempt: 0,
        offset: p.start,
        flushed_from: p.start,
        accounting: true,
    };
    pump.stream(p.first).await;
    pump.flush();
}

/// Releases the segment in the registry however the pump ends (even on panic).
struct EndGuard {
    dd: Arc<DdState>,
    id: String,
    epoch: u64,
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        self.dd.grants.end_segment(&self.id, self.epoch);
    }
}

/// The segment's cancel signal.
struct CancelWatch {
    rx: watch::Receiver<Cancel>,
    /// False once the sender is gone without a signal: the grant completed or vanished.
    /// The segment then runs to its end, unaccounted.
    live: bool,
}

impl CancelWatch {
    /// Resolves when the segment must stop (preempted or cancelled). Never resolves once
    /// the sender is gone without a signal. Cancel-safe.
    async fn stopped(&mut self) {
        while self.live {
            match self.rx.changed().await {
                Ok(()) => {
                    if *self.rx.borrow_and_update() != Cancel::No {
                        return;
                    }
                }
                Err(_) => self.live = false,
            }
        }
        std::future::pending::<()>().await
    }
}

struct Pump {
    dd: Arc<DdState>,
    id: String,
    kind: BackendKind,
    file: ResolvedFile,
    epoch: u64,
    end: u64,
    tx: mpsc::Sender<Body>,
    cancel: CancelWatch,
    throttle: Arc<Mutex<Throttle>>,
    stall: Duration,
    /// Longest upstream silence tolerated while we wait for bytes.
    idle: Duration,
    retries: u32,
    attempt: usize,
    /// Next byte to hand to the body.
    offset: u64,
    /// Start of the bytes handed over but not yet reported to the registry.
    flushed_from: u64,
    accounting: bool,
}

impl Pump {
    async fn stream(&mut self, first: reqwest::Response) {
        let mut upstream: Upstream = Box::pin(first.bytes_stream());
        while self.offset <= self.end {
            let next = tokio::select! {
                biased;
                _ = self.cancel.stopped() => return,
                _ = self.tx.closed() => return,
                n = tokio::time::timeout(self.idle, upstream.next()) => n,
            };
            match next {
                Ok(Some(Ok(chunk))) => {
                    if !self.forward(chunk).await {
                        return;
                    }
                }
                // Cut (error or early end) or silent upstream: resume from `offset`.
                Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                    // Release the failed connection before opening another one.
                    drop(upstream);
                    match self.reopen().await {
                        Some(u) => upstream = u,
                        None => return,
                    }
                }
            }
        }
    }

    /// Throttle and hand `chunk` (clipped to the segment) to the body, piece by piece.
    /// False when the segment must end.
    async fn forward(&mut self, mut chunk: Bytes) -> bool {
        let room = self.end - self.offset + 1;
        if chunk.len() as u64 > room {
            chunk.truncate(room as usize);
        }
        while !chunk.is_empty() {
            let piece = chunk.split_to(chunk.len().min(PIECE));
            let len = piece.len() as u64;
            let wait = self
                .throttle
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(len, self.dd.now_us());
            if wait > 0 {
                tokio::select! {
                    biased;
                    _ = self.cancel.stopped() => return false,
                    _ = tokio::time::sleep(Duration::from_micros(wait)) => {}
                }
            }
            let sent = tokio::select! {
                biased;
                _ = self.cancel.stopped() => return false,
                r = tokio::time::timeout(self.stall, self.tx.send(Ok(piece))) => {
                    matches!(r, Ok(Ok(())))
                }
            };
            if !sent {
                return false; // client gone, or stalled for `stall_timeout`
            }
            self.offset += len;
            if self.offset - self.flushed_from >= FLUSH && !self.flush() {
                return false;
            }
        }
        true
    }

    /// Upstream failed at `offset`: retry within the budget. `None` when the segment ends
    /// (budget spent, source changed, cancelled or client gone).
    async fn reopen(&mut self) -> Option<Upstream> {
        loop {
            self.dd.stats.record_upstream_error();
            if self.retries == 0 {
                self.fail("upstream failed").await;
                return None;
            }
            self.retries -= 1;
            let backoff = BACKOFF[self.attempt.min(BACKOFF.len() - 1)];
            self.attempt += 1;
            let (dd, file) = (&self.dd, &self.file);
            let (kind, offset, end) = (self.kind, self.offset, self.end);
            let opened = tokio::select! {
                biased;
                _ = self.cancel.stopped() => return None,
                _ = self.tx.closed() => return None,
                r = async {
                    tokio::time::sleep(backoff).await;
                    dd.backends.open(kind, file, offset, end).await
                } => r,
            };
            match opened {
                Ok(resp) => return Some(Box::pin(resp.bytes_stream())),
                Err(BackendError::SourceChanged) => {
                    self.flush();
                    self.accounting = false;
                    self.dd.grants.fail_source_changed(&self.id, now_ms());
                    self.fail("source file changed").await;
                    return None;
                }
                Err(_) => continue, // counted, consumes a retry
            }
        }
    }

    /// Abort the body: the client sees a truncated response and resumes.
    async fn fail(&mut self, why: &'static str) {
        let err = io::Error::other(why);
        let _ = tokio::time::timeout(self.stall, self.tx.send(Err(err))).await;
    }

    /// Report the bytes handed over since the last flush. False when the segment must
    /// stop (over-served).
    fn flush(&mut self) -> bool {
        if !self.cancel.live {
            self.accounting = false; // the grant is gone
        }
        if !self.accounting || self.offset <= self.flushed_from {
            return true;
        }
        let progress = self.dd.grants.record_progress(
            &self.id,
            self.epoch,
            self.flushed_from,
            self.offset,
            now_ms(),
        );
        self.flushed_from = self.offset;
        match progress {
            Progress::Continue => true,
            // Completed (or gone): deliver the rest of the range, nothing more to account.
            Progress::Complete => {
                self.accounting = false;
                true
            }
            Progress::OverServed => {
                self.accounting = false;
                false
            }
        }
    }
}

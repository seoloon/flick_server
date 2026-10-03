//! FlickDD analytics: totals, daily buckets, top titles, history ring, Prometheus export.
//!
//! Time is always passed in (`now_ms`, wall clock): nothing in here reads a clock.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write;
use std::sync::Mutex;

use serde::Serialize;

pub use super::types::BackendKind;
use crate::metrics::Counter;

const DAY_MS: u64 = 86_400_000;
/// Daily buckets kept (today included).
const DAYS: u64 = 30;
const HISTORY: usize = 500;
const TOP_TITLES: usize = 10;
/// Bound of the per-title aggregation map.
const MAX_TITLES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Cancelled,
    Expired,
    SourceChanged,
}

impl Outcome {
    pub const ALL: [Outcome; 4] = [
        Outcome::Completed,
        Outcome::Cancelled,
        Outcome::Expired,
        Outcome::SourceChanged,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Completed => "completed",
            Outcome::Cancelled => "cancelled",
            Outcome::Expired => "expired",
            Outcome::SourceChanged => "source_changed",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

fn backend_index(b: BackendKind) -> usize {
    b as usize
}

#[derive(Debug, Clone, Serialize)]
pub struct FinishedDownload {
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
    pub finished_at: u64,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, Default)]
struct DayBucket {
    bytes: u64,
    completed: u64,
}

#[derive(Debug, Clone)]
struct TitleAgg {
    title: Option<String>,
    kind: Option<String>,
    count: u64,
    bytes: u64,
    last_ms: u64,
}

#[derive(Default)]
struct StatsInner {
    /// Day index (`now_ms / DAY_MS`) -> bucket, last [`DAYS`] days only.
    days: BTreeMap<u64, DayBucket>,
    /// Newest first.
    history: VecDeque<FinishedDownload>,
    /// Completed downloads by `(backend, item_id)`, at most [`MAX_TITLES`] entries.
    titles: HashMap<(BackendKind, String), TitleAgg>,
}

/// Analytics registry. Totals are lock-free counters; the rest sits behind one short mutex.
#[derive(Default)]
pub struct Stats {
    bytes_by_backend: [Counter; 2],
    completed_by_backend: [Counter; 2],
    finished_by_backend: [Counter; 2],
    by_outcome: [Counter; 4],
    resumes: Counter,
    upstream_errors: Counter,
    rejected: Mutex<HashMap<&'static str, u64>>,
    inner: Mutex<StatsInner>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    /// Finished downloads, whatever the outcome.
    pub downloads: u64,
    pub completed: u64,
    pub bytes_served: u64,
    pub resumes: u64,
    pub upstream_errors: u64,
    pub rejected: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayStat {
    /// Day index since the Unix epoch (`ms / 86_400_000`).
    pub day: u64,
    /// Start of the day, ms since the Unix epoch (UTC).
    pub date_ms: u64,
    pub bytes: u64,
    pub completed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopTitle {
    pub backend: BackendKind,
    pub item_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendSplit {
    pub bytes: u64,
    pub downloads: u64,
    pub completed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    pub totals: Totals,
    /// [`DAYS`] entries (zero-filled), oldest first, ending today.
    pub days: Vec<DayStat>,
    pub top_titles: Vec<TopTitle>,
    pub by_backend: BTreeMap<&'static str, BackendSplit>,
    pub by_outcome: BTreeMap<&'static str, u64>,
    pub rejected: BTreeMap<&'static str, u64>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StatsInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bytes delivered to a client: daily bucket and per-backend totals.
    pub fn record_bytes(&self, backend: BackendKind, bytes: u64, now_ms: u64) {
        if bytes == 0 {
            return;
        }
        self.bytes_by_backend[backend_index(backend)].add(bytes);
        if let Some(d) = self.lock().day_mut(now_ms) {
            d.bytes += bytes;
        }
    }

    pub fn record_resume(&self) {
        self.resumes.inc();
    }

    pub fn record_upstream_error(&self) {
        self.upstream_errors.inc();
    }

    /// A request refused by a guard (e.g. "slots", "global", "rate", "quota").
    pub fn record_rejected(&self, reason: &'static str) {
        *self
            .rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(reason)
            .or_default() += 1;
    }

    /// A grant left the registry: history ring, outcome split, daily completions, top titles.
    pub fn record_finished(&self, f: FinishedDownload, now_ms: u64) {
        let b = backend_index(f.backend);
        self.by_outcome[f.outcome.index()].inc();
        self.finished_by_backend[b].inc();
        let mut inner = self.lock();
        if f.outcome == Outcome::Completed {
            self.completed_by_backend[b].inc();
            if let Some(d) = inner.day_mut(now_ms) {
                d.completed += 1;
            }
            inner.add_title(&f, now_ms);
        }
        inner.history.push_front(f);
        inner.history.truncate(HISTORY);
    }

    /// Finished downloads, newest first.
    pub fn history(&self) -> Vec<FinishedDownload> {
        self.lock().history.iter().cloned().collect()
    }

    fn totals(&self) -> Totals {
        let sum = |c: &[Counter]| c.iter().map(Counter::get).sum();
        Totals {
            downloads: sum(&self.by_outcome),
            completed: self.by_outcome[Outcome::Completed.index()].get(),
            bytes_served: sum(&self.bytes_by_backend),
            resumes: self.resumes.get(),
            upstream_errors: self.upstream_errors.get(),
            rejected: self.rejected_map().values().sum(),
        }
    }

    fn rejected_map(&self) -> BTreeMap<&'static str, u64> {
        self.rejected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect()
    }

    pub fn snapshot(&self, now_ms: u64) -> StatsSnapshot {
        let today = now_ms / DAY_MS;
        let (days, top_titles) = {
            let inner = self.lock();
            let days = (today.saturating_sub(DAYS - 1)..=today)
                .map(|day| {
                    let b = inner.days.get(&day).copied().unwrap_or_default();
                    DayStat {
                        day,
                        date_ms: day * DAY_MS,
                        bytes: b.bytes,
                        completed: b.completed,
                    }
                })
                .collect();
            let mut top: Vec<TopTitle> = inner
                .titles
                .iter()
                .map(|((backend, item_id), a)| TopTitle {
                    backend: *backend,
                    item_id: item_id.clone(),
                    title: a.title.clone(),
                    kind: a.kind.clone(),
                    count: a.count,
                    bytes: a.bytes,
                })
                .collect();
            top.sort_by(|x, y| (y.count, y.bytes, &x.item_id).cmp(&(x.count, x.bytes, &y.item_id)));
            top.truncate(TOP_TITLES);
            (days, top)
        };
        let by_backend = BackendKind::ALL
            .iter()
            .map(|&k| {
                let i = backend_index(k);
                let split = BackendSplit {
                    bytes: self.bytes_by_backend[i].get(),
                    downloads: self.finished_by_backend[i].get(),
                    completed: self.completed_by_backend[i].get(),
                };
                (k.as_str(), split)
            })
            .collect();
        let by_outcome = Outcome::ALL
            .iter()
            .map(|o| (o.as_str(), self.by_outcome[o.index()].get()))
            .collect();
        StatsSnapshot {
            totals: self.totals(),
            days,
            top_titles,
            by_backend,
            by_outcome,
            rejected: self.rejected_map(),
        }
    }

    /// Prometheus text exposition of the `flickdd_*` series.
    pub fn render_prometheus(&self, active: u64) -> String {
        let mut out = String::new();
        let mut series = |name: &str, kind: &str, help: &str, rows: Vec<(String, u64)>| {
            let _ = writeln!(out, "# HELP flickdd_{name} {help}");
            let _ = writeln!(out, "# TYPE flickdd_{name} {kind}");
            for (labels, v) in rows {
                let _ = writeln!(out, "flickdd_{name}{labels} {v}");
            }
        };
        series(
            "active_downloads",
            "gauge",
            "Download grants currently open",
            vec![(String::new(), active)],
        );
        series(
            "grants_total",
            "counter",
            "Finished download grants by outcome",
            Outcome::ALL
                .iter()
                .map(|o| {
                    let v = self.by_outcome[o.index()].get();
                    (format!("{{outcome=\"{}\"}}", o.as_str()), v)
                })
                .collect(),
        );
        series(
            "bytes_served_total",
            "counter",
            "Bytes delivered to download clients by backend",
            BackendKind::ALL
                .iter()
                .map(|&k| {
                    let v = self.bytes_by_backend[backend_index(k)].get();
                    (format!("{{backend=\"{}\"}}", k.as_str()), v)
                })
                .collect(),
        );
        series(
            "resumes_total",
            "counter",
            "Download segments that resumed a grant",
            vec![(String::new(), self.resumes.get())],
        );
        series(
            "upstream_errors_total",
            "counter",
            "Upstream (media server) stream failures",
            vec![(String::new(), self.upstream_errors.get())],
        );
        series(
            "rejected_total",
            "counter",
            "Download requests rejected by a guard, by reason",
            self.rejected_map()
                .into_iter()
                .map(|(r, v)| (format!("{{reason=\"{r}\"}}"), v))
                .collect(),
        );
        out
    }
}

impl StatsInner {
    /// Bucket of the day of `now_ms`, pruning to the last [`DAYS`] days. `None` when that
    /// day is already out of the window (a stale timestamp): totals still count it.
    fn day_mut(&mut self, now_ms: u64) -> Option<&mut DayBucket> {
        let day = now_ms / DAY_MS;
        let newest = self.days.keys().next_back().map_or(day, |&k| k.max(day));
        let oldest_kept = newest.saturating_sub(DAYS - 1);
        if self
            .days
            .first_key_value()
            .is_some_and(|(&k, _)| k < oldest_kept)
        {
            self.days.retain(|&k, _| k >= oldest_kept);
        }
        (day >= oldest_kept).then(|| self.days.entry(day).or_default())
    }

    fn add_title(&mut self, f: &FinishedDownload, now_ms: u64) {
        let key = (f.backend, f.item_id.clone());
        if !self.titles.contains_key(&key) && self.titles.len() >= MAX_TITLES {
            // Evict the lowest count, oldest first among equals.
            if let Some(victim) = self
                .titles
                .iter()
                .min_by_key(|(_, a)| (a.count, a.last_ms))
                .map(|(k, _)| k.clone())
            {
                self.titles.remove(&victim);
            }
        }
        let a = self.titles.entry(key).or_insert_with(|| TitleAgg {
            title: None,
            kind: None,
            count: 0,
            bytes: 0,
            last_ms: now_ms,
        });
        a.count += 1;
        a.bytes += f.served;
        a.last_ms = now_ms;
        if f.title.is_some() {
            a.title.clone_from(&f.title);
        }
        if f.kind.is_some() {
            a.kind.clone_from(&f.kind);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    fn fin(id: &str, item: &str, outcome: Outcome, served: u64, at: u64) -> FinishedDownload {
        FinishedDownload {
            download_id: id.to_owned(),
            user_id: "u".into(),
            user_name: "User".into(),
            backend: BackendKind::Jellyfin,
            item_id: item.to_owned(),
            title: Some(format!("Title {item}")),
            kind: Some("movie".into()),
            size: 100,
            covered: 100,
            served,
            segments: 1,
            resumes: 0,
            started_at: at,
            finished_at: at,
            outcome,
        }
    }

    #[test]
    fn daily_buckets_accumulate_by_day_index() {
        let s = Stats::new();
        let base = 100 * DAY;
        s.record_bytes(BackendKind::Jellyfin, 10, base);
        s.record_bytes(BackendKind::Plex, 5, base + DAY - 1);
        s.record_bytes(BackendKind::Jellyfin, 7, base + DAY);
        let at = base + DAY + 5;
        s.record_finished(fin("a", "x", Outcome::Completed, 100, at), at);
        let snap = s.snapshot(base + DAY);
        assert_eq!(snap.days.len(), 30);
        let today = snap.days.last().unwrap();
        assert_eq!((today.day, today.bytes, today.completed), (101, 7, 1));
        assert_eq!(today.date_ms, base + DAY);
        let yesterday = &snap.days[28];
        assert_eq!(
            (yesterday.day, yesterday.bytes, yesterday.completed),
            (100, 15, 0)
        );
        assert_eq!(snap.days[0].day, 72);
        assert_eq!(snap.totals.bytes_served, 22);
        assert_eq!(snap.by_backend["plex"].bytes, 5);
    }

    #[test]
    fn only_the_last_30_days_are_retained() {
        let s = Stats::new();
        for d in 0..40 {
            s.record_bytes(BackendKind::Plex, 1, d * DAY);
        }
        assert_eq!(s.inner.lock().unwrap().days.len(), 30);
        let snap = s.snapshot(39 * DAY);
        assert_eq!(snap.days.len(), 30);
        assert_eq!(snap.days[0].day, 10);
        assert!(snap.days.iter().all(|d| d.bytes == 1));
        // A stale timestamp (out of the window) is not resurrected as a bucket.
        s.record_bytes(BackendKind::Plex, 1, 2 * DAY);
        assert_eq!(s.inner.lock().unwrap().days.len(), 30);
        assert_eq!(s.snapshot(39 * DAY).totals.bytes_served, 41);
    }

    #[test]
    fn history_ring_keeps_the_500_newest_first() {
        let s = Stats::new();
        for i in 0..510u64 {
            s.record_finished(fin(&i.to_string(), "x", Outcome::Cancelled, 1, i), i);
        }
        let h = s.history();
        assert_eq!(h.len(), 500);
        assert_eq!(h[0].download_id, "509");
        assert_eq!(h[499].download_id, "10");
    }

    #[test]
    fn top_titles_rank_completed_downloads_by_count_then_bytes() {
        let s = Stats::new();
        let mut t = 0;
        let mut done = |item: &str, outcome, served| {
            t += 1;
            s.record_finished(fin(&format!("d{t}"), item, outcome, served, t), t);
        };
        for _ in 0..3 {
            done("x", Outcome::Completed, 10);
        }
        for _ in 0..2 {
            done("y", Outcome::Completed, 10);
            done("z", Outcome::Completed, 50);
        }
        for _ in 0..5 {
            done("w", Outcome::Expired, 10); // not completed: not a top title
        }
        for i in 0..12 {
            done(&format!("one{i}"), Outcome::Completed, 1);
        }
        let top = s.snapshot(t).top_titles;
        assert_eq!(top.len(), 10);
        let ids: Vec<&str> = top.iter().take(3).map(|e| e.item_id.as_str()).collect();
        assert_eq!(ids, ["x", "z", "y"]);
        assert_eq!((top[0].count, top[0].bytes), (3, 30));
        assert_eq!(top[0].title.as_deref(), Some("Title x"));
        assert!(top.iter().all(|e| e.item_id != "w"));
    }

    #[test]
    fn top_title_map_is_capped_evicting_the_oldest_lowest_count() {
        let s = Stats::new();
        s.record_finished(fin("h1", "hot", Outcome::Completed, 1, 0), 0);
        s.record_finished(fin("h2", "hot", Outcome::Completed, 1, 1), 1);
        for i in 0..999u64 {
            let at = 10 + i;
            s.record_finished(
                fin(
                    &format!("d{i}"),
                    &format!("i{i}"),
                    Outcome::Completed,
                    1,
                    at,
                ),
                at,
            );
        }
        assert_eq!(s.inner.lock().unwrap().titles.len(), 1000);
        s.record_finished(fin("new", "new", Outcome::Completed, 1, 5000), 5000);
        let inner = s.inner.lock().unwrap();
        assert_eq!(inner.titles.len(), 1000);
        let has = |item: &str| {
            inner
                .titles
                .contains_key(&(BackendKind::Jellyfin, item.to_owned()))
        };
        assert!(has("hot"), "higher count survives although older");
        assert!(!has("i0"), "oldest count-1 entry is evicted");
        assert!(has("i1") && has("new"));
    }

    #[test]
    fn renders_prometheus_lines() {
        let s = Stats::new();
        s.record_bytes(BackendKind::Plex, 5, 0);
        s.record_finished(fin("a", "x", Outcome::Completed, 5, 0), 0);
        s.record_resume();
        s.record_upstream_error();
        s.record_rejected("slots");
        let text = s.render_prometheus(3);
        for line in [
            "flickdd_active_downloads 3",
            "flickdd_grants_total{outcome=\"completed\"} 1",
            "flickdd_grants_total{outcome=\"expired\"} 0",
            "flickdd_bytes_served_total{backend=\"plex\"} 5",
            "flickdd_bytes_served_total{backend=\"jellyfin\"} 0",
            "flickdd_resumes_total 1",
            "flickdd_upstream_errors_total 1",
            "flickdd_rejected_total{reason=\"slots\"} 1",
            "# TYPE flickdd_grants_total counter",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in\n{text}"
            );
        }
    }
}

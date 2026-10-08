# Logs: in-memory log buffer and live log view: design

Sub-project 3 of 4 (see [2026-10-07-module-lifecycle-settings-design.md](2026-10-07-module-lifecycle-settings-design.md)):
the server keeps its latest log lines in memory and serves them on the admin API; the panel gets a live **Logs** page.
Sub-projects 1 (server core) and 2 (panel settings) are merged; everything here builds on them.

## 1. Scope

In scope: a bounded in-memory buffer fed by a `tracing` layer, `GET /admin/v1/logs`, the error code `INVALID_QUERY`,
the boot-only variable `FLICKSYNC_LOG_BUFFER`, the panel route `/api/logs`, the Logs page and its sidebar entry, and
the docs (`docs/admin-api.md`, `docs/deployment.md`, `.env.example`, `panel/README.md`).

Out of scope: persisting logs (they stay in memory, lost on restart), streaming (SSE / WebSocket), spans, log
shipping, server-side text search, the `.env` / compose rewrite (sub-project 4), any new crate or npm package.

## 2. Decisions

| Question | Decision |
|---|---|
| Buffer size | Boot-only `FLICKSYNC_LOG_BUFFER`: number of entries, default **2000**, allowed `0` to `10000`; `0` turns capture off (the layer is not installed). Out of range or not a number: configuration error at boot, like other boot variables. Not a panel setting: resizing a live buffer is not worth the code. |
| Memory bound | `message` is cut at **2048 bytes** and `fields` at **1024 bytes** (on a character boundary, then `…`). Worst case about 3.3 KiB per entry: 6.6 MB at 2000 entries, 33 MB at 10000; typical entries are under 300 bytes. |
| What an entry holds | `seq` (u64, from 1, +1 per entry, never reused within a run), `ts` (ms since the epoch, taken when the event is recorded), `level` (`error`/`warn`/`info`/`debug`/`trace`), `target` (module path), `message`, `fields` (the other fields as `name=value` separated by spaces; strings quoted like the console output, `%` values unquoted; fields named `log.*` from the `log` bridge are skipped). Spans are not recorded. |
| Which events | Exactly those the active filter lets through: the capture layer sits after the reloadable `EnvFilter` (`FLICKSYNC_LOG_LEVEL`), so a level change applied by `POST /settings/server/reload` applies to the buffer at once. `FLICKSYNC_LOG_FORMAT` does not matter. The invitation banner is printed with `eprint!`, not `tracing`, and never enters the buffer. |
| Secrets | Defence in depth (the code already logs no secret): (1) a field whose name, lowercased, contains `token`, `secret`, `password`, `passwd`, `api_key`, `apikey`, `authorization`, `cookie` or `credential`, or is `key` or ends in `_key` / `-key` / `.key`, is recorded as `name=<redacted>`; (2) in `message` and `fields`, the value after `Bearer ` and the value of any `name=value` whose name matches rule (1) (query strings, `key=value` text) is replaced by `<redacted>` up to the next `&`, whitespace, quote, `,`, `;`, `)`, `]`, `}` or `>`. `<redacted>` is the marker `Config`'s `Debug` already uses. Scrubbing runs before truncation. |
| Locking | `std::sync::Mutex<VecDeque<Arc<LogEntry>>>` plus the next `seq`, inside a `LogBuffer`. Formatting, scrubbing, truncation and the `Arc` allocation happen before the lock; the critical section assigns `seq`, pushes, and pops the oldest entry when full; the evicted entry is freed after the lock is released. A read takes the lock to compute the start index in O(1) (seqs in the buffer are contiguous), scan forward and clone at most `limit` `Arc`s; JSON is built after the lock. A poisoned lock is used as is (`into_inner`), like the rest of the code. |
| Why not something else | A lock-free queue needs a new crate and cannot be read by seq; an `RwLock` gains nothing because writers dominate; a `tokio::sync::broadcast` channel fits push streaming, not cursor polling. An uncontended futex lock costs tens of nanoseconds, below the cost of formatting the event, which the console layer already pays. |
| Route | `GET /admin/v1/logs?after=<seq>&level=<min>&limit=<n>` behind `AdminAuth`, `no-store` (existing admin layer). `after`: whole number, default `0`. `level`: minimum severity, case-insensitive, default `trace` (everything recorded). `limit`: `1` to `1000`, default `500`. Entries are returned oldest first. |
| Paging | The server scans entries with `seq > after` in order and returns the first `limit` that pass `level`. `next` is the seq of the last entry **scanned** (filtered-out ones included), or `after` when nothing was scanned; `more` is true when the scan stopped before the newest entry. A client sends `after = next` and repeats while `more` is true. |
| Lost lines | `dropped` = number of entries with `seq > after` already evicted (whatever their level): `first_seq - after - 1` when positive, else `0`. |
| Server restarts | Every answer carries `boot` (the run's start time in ms). A client that sees `boot` change starts again from `after=0`. A cursor above the newest seq of the same run returns no entries, `next = after`. |
| Query errors | `400 INVALID_QUERY` with a message naming the parameter, the value (shortened to 64 characters) and what is expected: unknown parameter (lists `after`, `level`, `limit`), a parameter given twice, `after` not a whole number, `level` not one of the five, `limit` outside 1 to 1000, or an unreadable query string. |
| Panel polling | Every **2 s** while the tab is visible, one request at a time, `after = cursor`, `limit = 1000`; while `more` is true (first load of a full buffer, catching up after a pause) it asks again at once, at most 10 pages per poll. All levels are fetched; the level filter and the text search are applied in the browser, so changing them is instant and covers what is already loaded. |
| Panel buffer | At most **5000** lines kept in the page; the oldest are removed first and the page says so. |
| Pause / Resume | Pause stops polling (the cursor is kept). Resume continues from the cursor; lines evicted meanwhile show as a gap marker. |
| Clear | Clears the view only (the cursor is kept, so only newer lines appear); the server's buffer is untouched. |
| Markers | A gap marker ("N lines were missed: the server keeps only its last C lines.") when `dropped > 0` on a poll that had a cursor above 0; a restart marker ("Flick Server restarted. The lines above are from its previous run.") when `boot` changes. Markers are hidden while a search is typed. |
| Times | Shown in UTC as `HH:MM:SS.mmm` (the console output is UTC too); copy and download use the full ISO time. |
| Copy / Download | The visible lines (after level and search) as text, one per line: `<ISO time> <LEVEL padded to 5> <target>: <message> <fields>`, markers in brackets. Download file `flick-server-logs-YYYYMMDD-HHMMSS.txt` (UTC). |
| Auto-scroll | A switch, on by default: after new lines the list scrolls to the bottom. Off: the list stays where the user left it. |
| Server unreachable | The last lines stay, a status pill reads **Reconnecting**, and an error notice shows `adminError`'s text (for `UNREACHABLE`: "Flick Server is unreachable. Check that it is running and that FLICKSYNC_URL points to it, then try again.") followed by "The lines below were received earlier. New lines appear again as soon as the server answers." Polling keeps going. |
| Capture off | `capacity: 0` in the answer: the page says the server keeps no log lines because `FLICKSYNC_LOG_BUFFER` is 0. |
| Navigation | Sidebar: Overview, FlickSync, FlickDD, **Logs**, Settings. Icon `scroll-text`. |
| Language and copy | English, the design system's content rules (Title Case buttons, sentence case text, British spelling, no emoji, no exclamation marks). |

## 3. Server

New module `src/logs.rs`:

- `Level` (`Trace < Debug < Info < Warn < Error`, serialised lowercase), `LogEntry`, `LogBuffer`
  (`new(capacity)`, `capacity()`, `boot()`, `push(level, target, message, fields) -> u64`, `query(&LogQuery) -> LogPage`),
  `LogQuery` (`after`, `min_level`, `limit`; `from_pairs` parses the query string and returns the `INVALID_QUERY`
  message on error), `LogPage` (`entries`, `next`, `more`, `dropped`).
- Redaction helpers: `is_secret_name`, `scrub`, `REDACTED`; truncation at `MAX_MESSAGE_BYTES` / `MAX_FIELDS_BYTES`.
  `push` applies them, so every way into the buffer is covered.
- `LogCapture`, a `tracing_subscriber::Layer` whose `on_event` collects the fields and calls `push`.

Wiring: `main.rs` builds `Arc<LogBuffer>` from the boot config, installs
`registry().with(reloadable EnvFilter).with(capacity > 0 then LogCapture).with(fmt layer)`, and passes the same buffer
to `AppState::with_log_buffer`, so start-up lines (module start or failure) are kept. `AppState::new` and
`with_clock` create their own buffer of the configured size (tests push into it directly). `AppState.logs` is public.

Answer of `GET /admin/v1/logs`:

```json
{
  "boot": 1790959000000, "capacity": 2000, "log_level": "info",
  "entries": [{ "seq": 41, "ts": 1790960000123, "level": "warn", "target": "flicksync::api::admin",
                "message": "admin API: invalid or missing token", "fields": "" }],
  "next": 41, "more": false, "dropped": 0
}
```

`log_level` is `FLICKSYNC_LOG_LEVEL` of the server settings in force.

## 4. Panel

| Piece | Content |
|---|---|
| `src/lib/types.ts` | `LogLevel`, `LogEntry`, `LogsResponse` |
| `src/lib/logs.ts` (pure, `node --test`) | levels and labels, constants (`POLL_MS`, `PAGE_LIMIT`, `MAX_CLIENT_LINES`, `MAX_PAGES_PER_POLL`), `LogLine` / `LogView`, `emptyView`, `logsPath`, `mergeLogs` (cursor, dedupe, gap and restart markers, trimming), `clearView`, `visibleLines`, `timeOfDay`, `formatEntry`, `markerText`, `lineText`, `linesToText`, `downloadName`, `liveState` / `LIVE_BADGE`, the page's texts, `parseLogsQuery` (proxy allow-list) |
| `src/app/api/logs/route.ts` | `GET` only; `hasSession()`; query checked by `parseLogsQuery` (only `after` and `limit` as digits and `level` as one of the five, each at most once; else `400 INVALID_QUERY` with a message); forwarded to `GET /admin/v1/logs`; `no-store`; the server's answer relayed as is |
| `src/lib/proxy.ts` | `invalidQuery(message)` |
| `src/lib/use-logs.ts` | the polling loop (`useLogs`) |
| `src/app/(panel)/logs/page.tsx`, `LogsView.tsx` | the page: header, toolbar (level segmented control, search, Pause / Resume, Auto-scroll switch, Copy Lines, Download, Clear View), status pill, notices, the line list (`role="log"`), a summary line |
| `src/lib/modules.ts`, `PanelSidebar.tsx`, `icons.tsx`, `panel.css` | `LOGS` nav entry, icons `scroll-text`, `pause`, `trash-2`, log list styles from existing tokens (`--warn`, `--error` for levels) |

## 5. Security

- The route is behind the admin token and the panel route behind the session, like every other admin read. The
  admin API is still never exposed by the reverse proxy.
- The buffer holds what the console already shows, minus redacted values; nothing new is logged by this feature, and
  the logs route itself logs nothing (no feedback loop).
- Bounded memory (capacity and per-entry caps), bounded answers (`limit` at most 1000), bounded panel memory (5000 lines).
- No new dependency on either side; no CSP change (download uses a `blob:` link the page creates).

## 6. Testing

- Unit (`src/logs.rs`): seq order and contiguity, cursor, level filter advancing `next` past filtered entries,
  `limit` and `more`, `dropped` after eviction, capacity 0, a cursor beyond the newest entry, query parsing and every
  `INVALID_QUERY` case, secret field names, `Bearer` and `token=` scrubbing, truncation on a character boundary, the
  capture following a filter reload, and concurrent pushes from several threads keeping seqs unique and contiguous.
- Config: `FLICKSYNC_LOG_BUFFER` default, `0`, the maximum, out of range and not a number; boot-only (catalogue drift test).
- Integration (`tests/logs.rs`): token required, `404` with the admin API off, `no-store` on success and error,
  paging end to end, `dropped` with a small buffer, `INVALID_QUERY` cases, no secret in any answer, a real event
  (a refused admin call) reaching the route through the capture layer.
- Panel (`node --test`): query allow-list, merge (first page, append, dedupe, gap only with a cursor, restart, trim),
  clear, filters, formatting, file name, live state. `tsc --noEmit` and `next build` clean. A manual pass against a
  local server covers the page.

## 7. Rollout

One commit per concern, each leaving `cargo fmt --check`, `cargo clippy --all-targets` and `cargo test` green (and
`npm test`, `npm run typecheck`, `npm run build` for panel commits): redaction helpers; buffer and query; boot
setting; capture layer and wiring; admin route and docs; panel logic; panel route; Logs page; final check.

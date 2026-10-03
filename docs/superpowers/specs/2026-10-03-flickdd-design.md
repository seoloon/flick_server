# FlickDD: design

Module of Flick Server that lets the Flick app download movies and episodes for
offline viewing, with precise control (parallelism, bandwidth) and analytics.

## 1. Goal and constraints

- The Flick client downloads a movie or an episode (one request per episode) to watch offline.
- **At most 10 downloads in parallel** per user, **10 MB/s maximum per download**.
- **Direct streaming**: no temporary copy, no disk, no buffering beyond one chunk.
- Sources: **Jellyfin and Plex**, configured server-side. The client never sees the source URL or credentials.
- The server collects analytics, exposed to the web panel and to Prometheus.
- **Unstable connections are expected**: downloads are fragmented and resumable, with guard rails on both sides.
- Deliverables: Rust module, panel module, and `docs/flickdd-integration.md` (client guide).

Assumptions: backend API keys are the server's own, so any Flick user holding the
`downloads:create` permission can download any library item (no per-user rights on the
backend). Files are served as-is (original file, no transcoding). Everything is in
memory, no database, like the rest of Flick Server.

## 2. Architecture

FlickDD is a streaming proxy:

```
Flick app --Range GET--> FlickSync --Range GET--> Jellyfin / Plex
          <--bytes------  (throttle)  <--bytes---
```

`reqwest` byte stream, then a per-grant token-bucket throttle, then `axum::body::Body::from_stream`.
TCP back-pressure propagates: a slow client slows the upstream read. Memory per active
segment is one chunk (<= 64 KiB). The 10 MB/s of each download pass through FlickSync, so
size the server's bandwidth accordingly.

Code layout (new, behind `FLICKDD_ENABLED`):

```
src/dd/mod.rs        DdState: config, grants, stats, backends
src/dd/backend.rs    trait MediaBackend + JellyfinBackend + PlexBackend (resolve + open range)
src/dd/grants.rs     grant registry: tokens, slots, coverage, expiry, preemption
src/dd/throttle.rs   token bucket (pure, takes the time as input)
src/dd/stats.rs      counters, daily buckets, history ring, Prometheus export
src/dd/stream.rs     the streaming body: throttle, stall guard, upstream retry, accounting
src/api/downloads.rs client routes
src/api/admin.rs     admin routes (dd/*)
```

When disabled, the routes answer 404 and nothing is allocated.

## 3. Client API

All under `/api/v1`. JSON errors use the existing `{error: {code, message}}` shape.

| Route | Auth | Purpose |
|---|---|---|
| `POST /downloads` | Flick JWT, perm `downloads:create` | Create a download grant |
| `GET /downloads/{id}` | download token | Status: size, bytes covered, expiry |
| `GET /downloads/{id}/file` | download token | The bytes. `Range` and `If-Range` supported |
| `DELETE /downloads/{id}` | download token | Cancel and free the slot |

The download token is `Authorization: Bearer <token>` or `?token=` (for OS background
downloaders that cannot set headers). It is an opaque 256-bit random value, stored hashed
(SHA-256) server-side, compared in constant time, never logged.

### Create

Request: `{ "backend": "jellyfin"|"plex", "item_id": "...", "title"?: "...", "kind"?: "movie"|"episode" }`.
`item_id` is validated against `[A-Za-z0-9_-]{1,64}` (no path injection towards the backend).

The server asks the backend for size, file name and mime type of the original file, then answers:

```json
{ "download_id": "...", "token": "...", "url": "/api/v1/downloads/{id}/file",
  "size": 4831838208, "filename": "Movie (2020).mkv", "mime": "video/x-matroska",
  "etag": "\"...\"", "chunk_bytes": 8388608, "max_range_bytes": 67108864,
  "rate_limit_bps": 10485760, "expires_at": 1790000000000 }
```

Errors: `403` missing permission, `404` item unknown to the backend, `429` already 10 active
downloads (or global cap), `502` backend unreachable, `503` FlickDD disabled for that backend.

### Slots

A grant occupies one slot from creation until it is completed, cancelled or expired.
A user holds at most `FLICKDD_MAX_PARALLEL` (10) slots; the server holds at most
`FLICKDD_MAX_GLOBAL` (default 100). A grant whose coverage reaches `size` is marked
completed and frees its slot at once.

### File and ranges

- `Range: bytes=a-b`, `bytes=a-`, no `Range` (whole file) are supported. Multi-range is refused (`416`).
- **Fragmentation by the server**: a response never carries more than `max_range_bytes`
  (default 64 MiB). A longer or open-ended range is answered `206` with a truncated, valid
  `Content-Range`; the client simply asks for the next fragment. The recommended client
  segment is `chunk_bytes` (8 MiB).
- `ETag` is derived from the grant's resolved file (backend, item, size, mtime when known).
  `If-Range` with a different ETag returns the full file (`200`), as HTTP specifies.
- `Accept-Ranges: bytes`, `Cache-Control: no-store`, `Content-Disposition` with the file name.

## 4. Throttling and concurrency

- **Rate**: one token bucket per grant, `FLICKDD_RATE_MBPS` (10 MB/s = 10 485 760 B/s),
  burst of 256 KiB (no initial burst of a full second, to keep the cap tight). Shared by all
  requests of the grant, so reconnecting never resets the allowance.
- **One active stream per grant**. A new request on a grant that has an active stream
  **preempts** the old one (old connection is closed). Reason: after a network change a dead
  TCP connection can linger for minutes; the client's retry must not be answered `409`.
- Per-grant request rate guard: `FLICKDD_MAX_REQUESTS_PER_MIN` (default 120); beyond it
  `429` with `Retry-After`. Stops reconnect storms.

## 5. Guard rails for unstable connections

Server side:

| Guard | Behaviour |
|---|---|
| Segment cut by the client | Segment recorded as `interrupted`; the grant, its slot and its coverage stay. Idle TTL is refreshed on every request. |
| Stall guard | If the client reads nothing for `FLICKDD_STALL_TIMEOUT` (default 30 s), the segment is aborted and the upstream connection released. The grant stays resumable. |
| Upstream failure mid-segment | Transparent retry against the backend from the exact current offset, up to `FLICKDD_UPSTREAM_RETRIES` (2) with 250 ms / 1 s backoff. If it still fails the body is ended abnormally (truncated vs declared `Content-Length`) so the client detects it and resumes. |
| Source changed | Every upstream response is checked against the grant (`Content-Range` total / `Content-Length` vs `size`). On mismatch the grant is invalidated: `409 SOURCE_CHANGED`, the client must create a new grant and restart that file. |
| Accurate accounting on resume | Served byte ranges are merged into a coverage interval set (small, bounded). Completion is `coverage == size`, so overlaps are not counted twice. |
| Over-serving cap | A grant may serve at most `FLICKDD_MAX_OVERSERVE` (default 2) x `size` in total, so a token cannot be abused as an endless download. Then `429 QUOTA_EXCEEDED`. |
| Lifetime | Idle TTL `FLICKDD_GRANT_TTL` (default 6 h), absolute cap `FLICKDD_GRANT_MAX_AGE` (default 24 h). The sweeper expires them and frees slots. |
| Upstream timeouts | Connect 5 s, first byte 15 s (`FLICKDD_UPSTREAM_TIMEOUT`). No total timeout (long downloads). |
| Shutdown | Active streams are closed during graceful shutdown; clients resume once the server is back (grants are in memory, so after a restart clients get `404` and recreate a grant, then resume with their local offset, see the client guide). |

Client side (documented in `docs/flickdd-integration.md`): segmented download by `Range`,
persist the byte offset on disk, resume with `If-Range` and the ETag, exponential backoff
with jitter honouring `Retry-After`, retry only on network errors / `5xx` / `429`, recreate the
grant on `404`/`409`, verify the final size, cap local parallelism to 10, pause on
connectivity loss and resume on reconnect, never trust a partial file without its offset record.

## 6. Analytics

Per download (live and history): user id, backend, item id, title, kind, size, covered bytes,
bytes served, number of segments and resumes, average and current speed, start, last activity,
duration, outcome (`completed`, `cancelled`, `expired`, `failed`, `source_changed`).

Aggregates: totals (downloads, completed, bytes served, resumes, failures), bytes and
completions per day for the last 30 days, top 10 titles, split by backend and by outcome.
History ring of the last 500 finished downloads.

Prometheus (on the existing `/metrics`): `flickdd_active_downloads`, `flickdd_grants_total{outcome}`,
`flickdd_bytes_served_total{backend}`, `flickdd_resumes_total`, `flickdd_upstream_errors_total`,
`flickdd_rejected_total{reason}`.

## 7. Admin API and panel

Same `AdminAuth` (admin bearer token, 404 when unset):

| Route | Returns |
|---|---|
| `GET /admin/v1/dd/overview` | enabled, backends configured, limits, totals, active count |
| `GET /admin/v1/dd/active` | live downloads with progress and speed |
| `GET /admin/v1/dd/history` | last finished downloads |
| `GET /admin/v1/dd/stats` | daily buckets, top titles, splits |
| `DELETE /admin/v1/dd/{id}` | cancel a download (closes its stream) |

Panel: new entry in `panel/src/lib/modules.ts` and page `panel/src/app/(panel)/flickdd/`
with panels: Overview (limits, totals), Active (progress bars, speed, cancel), History, Stats
(daily bytes chart, top titles). The existing admin proxy route is generalised to the new
resources; the admin token stays server-side.

## 8. Configuration

```
FLICKDD_ENABLED=false
FLICKDD_JELLYFIN_URL= / FLICKDD_JELLYFIN_API_KEY=
FLICKDD_PLEX_URL=     / FLICKDD_PLEX_TOKEN=
FLICKDD_MAX_PARALLEL=10        per user
FLICKDD_MAX_GLOBAL=100
FLICKDD_RATE_MBPS=10
FLICKDD_CHUNK_MB=8             advertised segment size
FLICKDD_MAX_RANGE_MB=64        server-side fragment cap
FLICKDD_GRANT_TTL=21600        seconds idle
FLICKDD_GRANT_MAX_AGE=86400
FLICKDD_STALL_TIMEOUT=30
FLICKDD_UPSTREAM_TIMEOUT=15
FLICKDD_UPSTREAM_RETRIES=2
FLICKDD_MAX_REQUESTS_PER_MIN=120
FLICKDD_MAX_OVERSERVE=2
```

`FLICKDD_ENABLED=true` requires at least one backend. Config follows the existing
`from_lookup` pattern with validation (`MAX_PARALLEL >= 1`, `RATE_MBPS > 0`, etc.).
New dependency: `reqwest` (`stream`, `rustls-tls`, no default features), `sha2`.
`Identity` gets a new permission constant `PERM_DOWNLOAD = "downloads:create"` (`*` already covers it).

## 9. Security

- Backend credentials stay in the server environment and are never logged or returned.
- Download tokens: random, hashed at rest, constant-time compare, bound to one grant and one user, never in logs (also not the `?token=` query: logged paths are scrubbed).
- `item_id` strictly validated; the backend path is built server-side.
- Redirects from the backend are not followed.
- Admin routes need the admin token; panel session required as for FlickSync.

## 10. Testing

- Unit: token bucket (injected time, exact rate and burst), grants (slots per user, global cap, expiry, preemption, over-serve cap), coverage interval merging, range parsing and truncation, config parsing and validation.
- Integration (real sockets, fake Jellyfin/Plex axum server): full create-then-download, `Range` and `If-Range`, truncated fragments, measured throughput within tolerance of the cap, 11th download rejected, cancel via DELETE and via admin, client disconnect then resume completes with accurate coverage, upstream cut mid-stream then transparent retry, source change gives 409, stall guard, preemption, flag disabled gives 404.
- Panel: unit tests for the new formatting helpers; manual check of the page against a local FlickSync.

## 11. Out of scope

Transcoding, per-user backend rights, persistence of grants across restarts, multi-range
responses, subtitles / extra tracks (can be added as further `item` kinds later),
server-side queueing of downloads (the client manages its own queue within the 10 slots).

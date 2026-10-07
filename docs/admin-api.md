# Admin API

Operator endpoints used by the [web panel](../panel/README.md). They are **off by default**: without
`FLICKSYNC_ADMIN_TOKEN` every `/admin/v1/*` path answers `404`.

* Set `FLICKSYNC_ADMIN_TOKEN` (at least 16 characters, e.g. `openssl rand -base64 32`).
* Every call needs `Authorization: Bearer <token>`, compared in constant time. Anything else is `401`
  (and counts in `auth_failures_total`).
* Every response of every `/admin/v1/*` route is `Cache-Control: no-store`: successes, `204`s and errors alike. There is no CORS: call it from a server, never from a browser, and
  keep the token out of client code. The panel's server does exactly that.
* One of these endpoints returns the invitation link, i.e. the signing key. Treat the token like the key.

## Endpoints

| Method and path | Purpose |
|---|---|
| `GET /admin/v1/overview` | Version, uptime, readiness, rooms, participants, connections and their limits, average RTT |
| `GET /admin/v1/invite` | The invitation link and how it was built |
| `GET /admin/v1/rooms` | Every live room with participants and playback, newest first |
| `DELETE /admin/v1/rooms/{room_id}` | Force-close a room: `204`, or `404` when it does not exist (any id form is accepted) |
| `GET /admin/v1/stats` | Counters, drift distribution and a rolling history |
| `GET /admin/v1/dd/overview` | FlickDD: enabled backends, limits, active count, headline totals |
| `GET /admin/v1/dd/active` | FlickDD: downloads in progress |
| `GET /admin/v1/dd/history` | FlickDD: recently finished downloads, newest first |
| `GET /admin/v1/dd/stats` | FlickDD: totals, 30 daily buckets, top titles, splits by backend and outcome |
| `DELETE /admin/v1/dd/{id}` | FlickDD: stop a download and free its slot |

### `GET /admin/v1/invite`

```json
{
  "url": "flicksync://sync.example.com/?v=1&tls=1#k=...",
  "address": "https://sync.example.com",
  "tls": true,
  "address_guessed": false,
  "key_source": "file",
  "key_count": 1,
  "kid": "main",
  "server_id": "default",
  "qr": ["1111111010...", "..."]
}
```

`address_guessed` is true when `FLICKSYNC_PUBLIC_URL` is unset (the address is the bind address). `key_source` is
`"file"` for the auto-generated key and `"environment"` for `FLICKSYNC_AUTH_KEYS[_FILE]`. `qr` holds one string per
row, `"1"` for a dark module and `"0"` for a light one, so a client can draw the code itself (add a quiet zone of 4
modules). The link format is specified in [flick-integration.md](flick-integration.md#invitation-link).

### `GET /admin/v1/rooms`

```json
{
  "now": 1790960000000,
  "rooms": [{
    "room_id": "K7M2Q9XP4TWB", "share_code": "K7M2-Q9XP-4TWB",
    "state": "playing", "host_id": "alice", "control_mode": "everyone", "chat_enabled": true,
    "max_participants": 100,
    "participants": [{ "participant_id": "alice", "display_name": "Alice", "presence": "connected",
                       "is_host": true, "joined_at": 1790959000000, "rtt_ms": 42.0 }],
    "media_title": "The Long Night", "media_provider": "jellyfin", "media_type": "movie",
    "playback": { "state": "playing", "position": 1520.0, "rate": 1.0, "server_time": 1790959999000, "sequence": 2 },
    "created_at": 1790959000000, "age_secs": 1000, "idle_secs": 3
  }]
}
```

`state` is one of `waiting`, `media_selected`, `playing`, `paused`, `empty`. `idle_secs` is the time since the room
last received a client message. `rtt_ms` is `null` until a client has reported one. A playing room's `position` is the
value at `playback.server_time`; add `(now - server_time) / 1000 * rate` for the current one.

Closing a room sends `room_closed` with `reason: "admin_closed"` to everyone in it and then closes their sockets
(code `4003`), exactly like a host closing it.

### `GET /admin/v1/stats`

```json
{
  "now": 1790960000000, "uptime_secs": 3600, "rtt_avg_ms": 41.2,
  "totals": { "rooms_created": 12, "rooms_destroyed": 9, "messages_in": 50321, "malformed_messages": 0,
              "rate_limited": 0, "auth_failures": 1, "sync_reports": 4800, "sync_corrections": 130, "sync_seeks": 2 },
  "drift": { "thresholds_ms": { "ignore": 100, "soft": 500, "hard": 1500 }, "buckets": [4100, 500, 190, 10] },
  "history_interval_secs": 10,
  "history": [{ "t": 1790959990000, "rooms": 3, "participants": 6, "connections": 5, "rtt_ms": 41.2,
                "messages_in": 50300, "corrections": 129, "seeks": 2, "reports": 4790 }]
}
```

* `totals` are counters since startup. `sync_corrections` counts every correction; `sync_seeks` is the share that
  were hard seeks, the rest being speed adjustments.
* `drift.buckets` counts evaluated sync reports by absolute drift: under `ignore`, under `soft`, under `hard`,
  `hard` and over. Reports from buffering clients and stale ones are not evaluated.
* `history` is a ring of at most 360 samples, one every 10 s (one hour), kept in memory and lost on restart. Its
  values are cumulative counters or gauges; derive rates from consecutive samples.

The same counters (plus `sync_seeks_total` and `sync_reports_total`) are available in Prometheus form on `/metrics`.

## FlickDD

The five `dd` routes expose [FlickDD](flickdd-integration.md) downloads. While FlickDD is off
(`FLICKDD_ENABLED=false`) all five answer `404` with `{"error":{"code":"DOWNLOAD_NOT_FOUND","message":"FlickDD is disabled"}}`,
so the panel can tell "off" from "no data". Download tokens are never part of any response. `user_id` is the key
`"{server_id}/{user_id}"` (a user id is only unique per Flick server), `user_name` the display name from the token.

### `GET /admin/v1/dd/overview`

```json
{
  "enabled": true,
  "backends": { "jellyfin": true, "plex": false },
  "limits": { "max_parallel": 10, "max_global": 100, "rate_bps": 10485760, "chunk_bytes": 8388608,
              "max_range_bytes": 67108864, "grant_ttl_secs": 21600 },
  "active": 2,
  "totals": { "downloads": 41, "completed": 37, "bytes_served": 96636764160, "resumes": 212,
              "upstream_errors": 3, "rejected": 5 }
}
```

`totals.downloads` counts finished downloads whatever their outcome; `rejected` is the number of creations or
requests refused for a limit (see `rejected` in `dd/stats`). All totals are counters since startup.

### `GET /admin/v1/dd/active`

```json
{
  "now": 1790960000000,
  "downloads": [{
    "download_id": "Qm9v...", "user_id": "my-flick/alice", "user_name": "Alice", "backend": "jellyfin",
    "item_id": "a1b2c3d4", "title": "The Long Night", "kind": "movie",
    "size": 4831838208, "covered": 83886080, "served": 92274688, "segments": 11, "resumes": 10,
    "started_at": 1790959000000, "last_activity": 1790959999000, "speed_bps": 10485760, "streaming": true
  }]
}
```

Oldest first. `covered` counts distinct bytes delivered, `served` counts every byte (re-reads included; a download
ends at `size x FLICKDD_MAX_OVERSERVE`). `segments` is the number of file requests, `resumes` those after the first.
`speed_bps` is the throughput over the last 5 seconds. `streaming` is true while a response is open. `title` and
`kind` are `null` when the client did not send them.

### `GET /admin/v1/dd/history`

`{ "now": ..., "downloads": [...] }`: the last 500 finished downloads (in memory, lost on restart), newest first. Each
entry has the fields of an active one except `speed_bps`, `streaming` and `last_activity`, plus `finished_at` and
`outcome`: `completed`, `cancelled`, `expired` (idle or too old, or its over-serve allowance was spent) or
`source_changed` (the file changed on the media server, or vanished).

### `GET /admin/v1/dd/stats`

```json
{
  "now": 1790960000000,
  "totals": { "downloads": 41, "completed": 37, "bytes_served": 96636764160, "resumes": 212, "upstream_errors": 3, "rejected": 5 },
  "days": [{ "day": 20723, "date_ms": 1790899200000, "bytes": 10737418240, "completed": 4 }],
  "top_titles": [{ "backend": "jellyfin", "item_id": "a1b2c3d4", "title": "The Long Night", "kind": "movie",
                   "count": 6, "bytes": 28991029248 }],
  "by_backend": { "jellyfin": { "bytes": 90000000000, "downloads": 30, "completed": 28 },
                  "plex": { "bytes": 6636764160, "downloads": 11, "completed": 9 } },
  "by_outcome": { "completed": 37, "cancelled": 2, "expired": 2, "source_changed": 0 },
  "rejected": { "global": 0, "slots": 3, "rate": 2, "quota": 0 }
}
```

* `days` always has 30 entries (zero-filled), oldest first, ending today; `date_ms` is the start of the day in UTC.
* `top_titles` lists the 10 most completed items.
* `rejected` counts refusals by reason: `slots` (per-user limit), `global` (server limit), `rate` (request rate guard),
  `quota` (over-serve allowance).

The same numbers are exported on `/metrics` as `flickdd_active_downloads`, `flickdd_grants_total{outcome}`,
`flickdd_bytes_served_total{backend}`, `flickdd_resumes_total`, `flickdd_upstream_errors_total` and
`flickdd_rejected_total{reason}` when metrics are enabled.

### `DELETE /admin/v1/dd/{id}`

Cancels the download `id` (its `download_id`): the running stream is cut, the slot is freed and the download appears
in the history as `cancelled`. `204`, or `404 DOWNLOAD_NOT_FOUND` when no such download is in progress. The client
sees its body truncated and its next request on the grant gets `404`.

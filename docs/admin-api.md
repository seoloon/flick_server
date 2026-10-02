# Admin API

Operator endpoints used by the [web panel](../panel/README.md). They are **off by default**: without
`FLICKSYNC_ADMIN_TOKEN` every `/admin/v1/*` path answers `404`.

* Set `FLICKSYNC_ADMIN_TOKEN` (at least 16 characters, e.g. `openssl rand -base64 32`).
* Every call needs `Authorization: Bearer <token>`, compared in constant time. Anything else is `401`
  (and counts in `auth_failures_total`).
* Responses are `Cache-Control: no-store`. There is no CORS: call it from a server, never from a browser, and
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

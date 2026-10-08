# Admin API

Operator endpoints used by the [web panel](../panel/README.md). Disabled (a bare `404`, no body) unless `PANEL_PASSWORD` has 10+
characters (or the legacy `FLICKSYNC_ADMIN_TOKEN` alone is set); the token is `hex(HMAC-SHA256(PANEL_PASSWORD, "flick-admin-api-v1"))`. The old `FLICKSYNC_ADMIN_TOKEN` still
works and is deprecated.

* Password length is measured in bytes server-side, while the panel counts characters; `PANEL_PASSWORD` is trimmed
  server-side, so derive the token from the trimmed value.
* Every call needs `Authorization: Bearer <token>`, compared in constant time. Anything else is
  `401 UNAUTHENTICATED` (and counts in `auth_failures_total`); the message tells a missing token from a refused one.
* Every response of every `/admin/v1/*` route is `Cache-Control: no-store`: successes, `204`s and errors alike. There is no CORS: call it from a server, never from a browser, and
  keep the token out of client code. The panel's server does exactly that.
* One of these endpoints returns the invitation link, i.e. the signing key. Treat the token like the key.
* Do not expose `/admin` through the reverse proxy.

## Errors

Every error has the body `{"error": {"code": "SETTINGS_INVALID", "message": "..."}}`. `code` is stable: branch on it.
`message` is an English sentence meant to be shown as is in the panel: it says what happened and what to do, names the
setting involved and the expected values, and may change between versions. It never contains a secret value or a
filesystem path (only the file names `settings.json` and `auth_keys`). The server logs keep the technical details.

| Code | HTTP | Returned by | Meaning |
|---|---|---|---|
| `UNAUTHENTICATED` | 401 | every route | No `Authorization: Bearer` token, or a token not derived from the server's `PANEL_PASSWORD` |
| `UNKNOWN_SCOPE` | 404 | `GET`/`PUT /settings/{scope}` | The scope is not `server`, `flicksync` or `flickdd`; the message lists the valid ones |
| `UNKNOWN_MODULE` | 404 | `POST /modules/{id}/{action}` | The module is not `flicksync` or `flickdd`; the message lists the valid ones |
| `UNKNOWN_ACTION` | 404 | `POST /modules/{id}/{action}` | The action is not `start`, `stop` or `reload`; the message lists the valid ones |
| `INVALID_PAYLOAD` | 400 | `PUT /settings/{scope}` | The body is not JSON, lacks `Content-Type: application/json`, is not `{"values": {...}}`, or a value is an array or an object |
| `UNKNOWN_SETTING` | 400 | `PUT /settings/{scope}` | A name is not a setting of this scope: unknown, boot-only (environment only), or of another scope (the message says which) |
| `SETTINGS_INVALID` | 400 | `PUT /settings/{scope}`, `POST /settings/server/reload` | A value, or the merged result, fails validation (the message names the setting, what is wrong and what is expected); nothing was written. On a reload: the stored server settings are invalid, the previous ones stay in force |
| `SETTINGS_WRITE_FAILED` | 500 | `PUT /settings/{scope}`, `POST /modules/{id}/start` and `/stop` | `settings.json` in the data directory cannot be written (read-only or full volume, permissions); nothing changed |
| `RELOAD_FAILED` | 500 | `POST /settings/server/reload` | The settings are valid but cannot be applied: the signing key file `auth_keys` (or the file named by `FLICKSYNC_AUTH_KEYS_FILE`) cannot be read or created, is empty or has an invalid entry. The previous settings stay in force |
| `MODULE_DISABLED` | 503 | `GET /rooms`, `DELETE /rooms/{id}`, `GET /stats` | FlickSync is not running: the message says whether it is stopped or failed to start, and why |
| `ROOM_NOT_FOUND` | 404 | `DELETE /rooms/{id}` | No live room has this id |
| `DOWNLOAD_NOT_FOUND` | 404 | `/dd/*` | FlickDD is not running (stopped, or failed to start: the message says why), or `DELETE /dd/{id}`: no such download in progress |
| `INTERNAL` | 500 | `GET /invite`, `GET /dd/stats` | The invitation cannot be built from the key in force, or an unexpected condition; details in the server logs |

A module that fails to start is not an HTTP error: the action answers `200` with `state: "failed"` and the reason in
the status `message`, written the same way (setting, problem, expected value, then what to do).

## Endpoints

| Method and path | Purpose |
|---|---|
| `GET /admin/v1/overview` | Version, uptime, readiness, rooms, participants, connections and their limits, average RTT. While FlickSync is stopped `running` is false and the counters are zero |
| `GET /admin/v1/invite` | The invitation link and how it was built |
| `GET /admin/v1/rooms` | (`503 MODULE_DISABLED` while FlickSync is stopped) Every live room with participants and playback, newest first |
| `DELETE /admin/v1/rooms/{room_id}` | Force-close a room: `204`, or `404` when it does not exist (any id form is accepted) |
| `GET /admin/v1/stats` | (`503 MODULE_DISABLED` while FlickSync is stopped) Counters, drift distribution and a rolling history |
| `GET /admin/v1/dd/overview` | FlickDD: enabled backends, limits, active count, headline totals |
| `GET /admin/v1/dd/active` | FlickDD: downloads in progress |
| `GET /admin/v1/dd/history` | FlickDD: recently finished downloads, newest first |
| `GET /admin/v1/dd/stats` | FlickDD: totals, 30 daily buckets, top titles, splits by backend and outcome |
| `DELETE /admin/v1/dd/{id}` | FlickDD: stop a download and free its slot |
| `GET /admin/v1/modules` | State of each module: `stopped`, `running` or `failed` (with the reason), the persisted `enabled` switch, `pending_reload` |
| `POST /admin/v1/modules/{id}/start` | Persist `enabled=true` and start (`flicksync` or `flickdd`) |
| `POST /admin/v1/modules/{id}/stop` | Persist `enabled=false` and stop: rooms close, downloads are cut |
| `POST /admin/v1/modules/{id}/reload` | Stop then start with the stored settings; a disabled module stays stopped |
| `GET /admin/v1/settings/{server\|flicksync\|flickdd}` | Every editable setting with value, source (`panel`, `environment`, `default`), default; secrets only as `set` |
| `PUT /admin/v1/settings/{scope}` | Partial update `{"values": {"NAME": value-or-null}}`; the result is validated as a whole, `400` and nothing written otherwise |
| `POST /admin/v1/settings/server/reload` | Apply the server scope (keys, public address, CORS, metrics, log level) without restarting modules |

### `GET /admin/v1/invite`

```json
{
  "url": "flickserver://sync.example.com/?v=1&tls=1#k=...",
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

## Modules and settings

FlickSync and FlickDD are **off by default** and are started, stopped and configured at runtime. While a module is
stopped its public routes stay mounted and answer `503 MODULE_DISABLED` (FlickSync's `/api/v1/rooms*`, for example);
stopping closes rooms and cuts running downloads. Settings live in `/data/settings.json`; stored values beat the
environment.

### `GET /admin/v1/modules`

```json
{ "modules": [
  { "id": "flicksync", "state": "running", "message": null, "enabled": true, "pending_reload": false, "since": 1790959000000 },
  { "id": "flickdd", "state": "failed", "message": "FlickDD needs at least one backend: set FLICKDD_JELLYFIN_URL and FLICKDD_JELLYFIN_API_KEY, or FLICKDD_PLEX_URL and FLICKDD_PLEX_TOKEN. Fix this setting, then reload the module.", "enabled": true, "pending_reload": false, "since": null }
] }
```

`state` is `stopped`, `running` or `failed`. `message` is always present: the reason when failed, otherwise `null`.
`since` is the start time in ms while running, otherwise `null`. `pending_reload` is true when stored settings
changed since the module started. Every status object (here and in the answers below) has these fields.

### `POST /admin/v1/modules/{id}/start`, `/stop`, `/reload`

No body. Answer `200`: the module's status object as above. `start` persists `enabled=true` first, `stop` persists
`enabled=false`; both are idempotent. `reload` restarts with the stored settings and leaves a disabled module stopped.
Invalid stored settings are not an HTTP error: the answer is `200` with `state: "failed"` and the reason in `message`.
A module that fails this way (FlickDD enabled without a backend, for instance) never affects the other module, the
server scope or the next boot. Stopping or reloading FlickDD resets its stats and history.
An unparseable `FLICKDD_ENABLED` in the environment (`perhaps`, say) counts as on: FlickDD is `failed` with a message
naming the variable rather than silently `stopped`; `stop`, or a `PUT` of a valid value, stores a switch that wins
over the environment. (An unparseable `FLICKSYNC_ENABLED` stops the process at boot, like any invalid shared setting.)
A `PUT` never stores an unparseable switch.
Errors: `404 UNKNOWN_MODULE` or `404 UNKNOWN_ACTION`, `500 SETTINGS_WRITE_FAILED` when `settings.json` cannot be
written (the module is then left as it was), `401 UNAUTHENTICATED` without the token.

### `GET /admin/v1/settings/{scope}`

`scope` is `server`, `flicksync` or `flickdd` (anything else: `404 UNKNOWN_SCOPE`). Answer:

```json
{ "scope": "flicksync", "revision": 3, "fields": [
  { "name": "FLICKSYNC_MAX_ROOMS", "kind": "int", "secret": false, "value": "10000", "set": false,
    "source": "default", "default": "10000" },
  { "name": "FLICKSYNC_DEFAULT_CONTROL_MODE", "kind": "choice", "secret": false, "value": "everyone", "set": false,
    "source": "default", "default": "everyone", "choices": ["everyone", "host_only"] }
] }
```

`kind` is `bool`, `int`, `float`, `text`, `choice`, `list` or `secret`. `value` and `default` are strings. `source` is
`panel`, `environment` or `default`; `set` says a value exists in the panel or the environment. `choices` appears for
`choice` fields only. For secrets `value` and `default` are always `null`, so only `set` (a boolean) tells whether one
exists.

### `PUT /admin/v1/settings/{scope}`

```json
{ "values": { "FLICKSYNC_MAX_ROOMS": 200, "FLICKSYNC_DEFAULT_CONTROL_MODE": null } }
```

Partial update: a name with a value stores it (strings, numbers and booleans are all accepted as scalars), `null`
removes the stored value (back to environment or default), an omitted name is untouched. The merged result is
validated as a whole and nothing is written on failure. Answer `200`: the new settings view. Changes to a running
module show as `pending_reload` until it is reloaded. Errors (nothing is written for any of them):
`400 INVALID_PAYLOAD` for a body that is not `{"values": {...}}` or a non-scalar value; `400 UNKNOWN_SETTING` for an
unknown field, a boot-only one or one that belongs to another scope (for example `FLICKSYNC_METRICS_TOKEN` is a
`server` setting); `400 SETTINGS_INVALID` for a value that fails validation, for example
`FLICKSYNC_MAX_ROOM_SIZE has an invalid value 'ten' (invalid digit found in string). Expected a whole number (0 or more). Nothing was saved.`;
`404 UNKNOWN_SCOPE`; `500 SETTINGS_WRITE_FAILED` when `settings.json` cannot be written.

### `POST /admin/v1/settings/server/reload`

Re-reads the server scope and applies it live: signing keys, public address, CORS origins, metrics and log level.
Modules keep running. Answer `200`: `{"reloaded": true}`. On failure the previous settings stay active: an invalid
stored configuration answers `400 SETTINGS_INVALID`, a signing key file that cannot be read, created or used answers
`500 RELOAD_FAILED`.

## FlickDD

The five `dd` routes expose [FlickDD](flickdd-integration.md) downloads. While the FlickDD module is stopped
(or failed to start) all five answer `404` with
`{"error":{"code":"DOWNLOAD_NOT_FOUND","message":"FlickDD is stopped. Start it with POST /admin/v1/modules/flickdd/start."}}`
(when it failed to start, the message gives the reason instead), so the panel can tell "off" from "no data". Download tokens are never part of any response. `user_id` is the key
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
requests refused for a limit (see `rejected` in `dd/stats`). All totals are counters since FlickDD last started:
stats and history (`dd/stats`, `dd/history`) are reset on every stop or reload of the module.

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
  "rejected": { "create": 0, "global": 0, "slots": 3, "rate": 2, "quota": 0 }
}
```

* `days` always has 30 entries (zero-filled), oldest first, ending today; `date_ms` is the start of the day in UTC.
* `top_titles` lists the 10 most completed items.
* `rejected` counts refusals by reason: `create` (creation rate limit, 30 per minute per user), `slots` (per-user
  limit), `global` (server limit), `rate` (request rate guard), `quota` (over-serve allowance).

The same numbers are exported on `/metrics` as `flickdd_active_downloads`, `flickdd_grants_total{outcome}`,
`flickdd_bytes_served_total{backend}`, `flickdd_resumes_total`, `flickdd_upstream_errors_total` and
`flickdd_rejected_total{reason}` when metrics are enabled.

### `DELETE /admin/v1/dd/{id}`

Cancels the download `id` (its `download_id`): the running stream is cut, the slot is freed and the download appears
in the history as `cancelled`. `204`, or `404 DOWNLOAD_NOT_FOUND` when no such download is in progress. The client
sees its body truncated and its next request on the grant gets `404`.

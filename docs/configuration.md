# Configuration reference

Flick Server is set up in the Flick Panel: modules and their settings are stored in
`settings.json` on the `/data` volume. The variables below are the **fallback**: they give the
default of each setting until the panel stores a value (the panel's value wins), and a few of them
are read at boot only. `.env.example` lists the ones a first start needs.

Boot-only (not changeable from the panel): `FLICKSYNC_HOST`, `FLICKSYNC_PORT`, `FLICKSYNC_DATA_DIR`,
`FLICKSYNC_LOG_FORMAT`, `FLICKSYNC_LOG_BUFFER`, `FLICKSYNC_AUTH_KEYS_FILE`, `FLICKSYNC_SHUTDOWN_GRACE`,
`FLICKSYNC_SWEEP_INTERVAL_MS`, `FLICKSYNC_MAX_BODY_BYTES`, `PANEL_PASSWORD`.

A leftover `FLICKSYNC_ADMIN_TOKEN` is ignored: the admin API token is derived from `PANEL_PASSWORD`
(see [admin-api.md](admin-api.md)).

Defaults below are the ones the code applies when a variable is unset. "unset" means no value.

## System

Process-wide: network, logs, storage, metrics.

### Network

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_HOST` | `0.0.0.0` | Address to bind. Boot-only. |
| `FLICKSYNC_PORT` | `8787` | Port to bind. Boot-only. |

### Logging

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_LOG_LEVEL` | `info` | tracing filter, e.g. `info` or `info,flicksync::room=debug`. |
| `FLICKSYNC_LOG_FORMAT` | `pretty` | `pretty` or `json`. Boot-only. |
| `FLICKSYNC_LOG_BUFFER` | `2000` | Log lines kept in memory for the panel's Logs page (0 = none, at most 10000). Lost on restart. Boot-only. |

### Storage

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_DATA_DIR` | `./data` | Where the generated signing key lives (file `auth_keys`, mode 0600) and `settings.json`. Keep it on a volume. Boot-only. |

### Metrics

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_METRICS_ENABLED` | `false` | Prometheus text on `GET /metrics`. |
| `FLICKSYNC_METRICS_TOKEN` | unset | If set, `/metrics` needs `Authorization: Bearer <token>`. |

### Internals

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_SWEEP_INTERVAL_MS` | `1000` | Interval between maintenance passes (expiry, heartbeats), in milliseconds. Boot-only. |
| `FLICKSYNC_SHUTDOWN_GRACE` | `10` | Seconds to let sockets drain on shutdown. Boot-only. |
| `FLICKSYNC_MAX_BODY_BYTES` | `16384` | Largest HTTP request body accepted, in bytes. Boot-only. |

## Web panel

Docs: `panel/README.md`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `ENABLE_WEB_PANEL` | `false` | Master switch. When false the panel container exits at once and nothing listens. (`docker-compose.yml` turns the panel on.) |
| `PANEL_PASSWORD` | unset | Sign-in password. Required when the panel is enabled. 10+ characters: `openssl rand -base64 18`. The panel and FlickSync derive the admin API token from it (see [admin-api.md](admin-api.md)). Boot-only. |

## FlickSync (Watch Together)

### Start at boot

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_ENABLED` | `false` | Start FlickSync at boot. The panel can also switch it (stored in `/data/settings.json`, which wins over this). |

### Access: invitation link and signing key

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_PUBLIC_URL` | unset | Public address clients use, e.g. `https://sync.example.com`. Needed behind a reverse proxy, where the server cannot know it. Without it the invitation uses the bind address over plain http. When a proxy serves FlickSync under a path prefix and strips it, include the prefix: `https://flick.example.com/services`. |

Show the invitation: `docker compose exec flick-modules flicksync invite`.
Rotate the key: `flicksync invite --rotate` (then restart).

### Access: your own keys (optional)

Unset = FlickSync generates and stores a key. Setting either of the first two variables disables that.
Format, comma or newline separated: `kid:server_id:secret`.

- `kid`: key id, sent in the JWT header.
- `server_id`: the Flick Server that owns the key; tokens must carry the same claim.
- `secret`: 32+ characters: `openssl rand -base64 48`.

Rotation: add a second key (same `server_id`), switch the issuer, remove the old one.

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_AUTH_KEYS` | unset | Your own keys, e.g. `main:my-flick-server:CHANGE-ME-use-at-least-32-random-characters`. |
| `FLICKSYNC_AUTH_KEYS_FILE` | unset | Same, from a file with one key per line (Docker/Kubernetes secrets), e.g. `/run/secrets/flicksync_keys`. Boot-only. |
| `FLICKSYNC_AUTH_AUDIENCE` | `flicksync` | Required `aud` claim. |
| `FLICKSYNC_AUTH_MAX_TOKEN_TTL` | `86400` | Refuse tokens that expire further ahead than this (seconds). |
| `FLICKSYNC_AUTH_LEEWAY` | `30` | Clock skew tolerance (seconds). |

### Rooms

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_MAX_ROOM_SIZE` | `100` | Participants per room, reconnecting ones included. |
| `FLICKSYNC_MAX_ROOMS` | `10000` | Rooms alive at once. |
| `FLICKSYNC_ROOM_CREATE_PER_MINUTE` | `6` | Rooms one user may create per minute (also the burst). |
| `FLICKSYNC_ROOM_TIMEOUT` | `60` | Seconds an empty room lives before it is deleted. |
| `FLICKSYNC_ROOM_IDLE_TIMEOUT` | `43200` | Seconds without any client message before a room expires. |
| `FLICKSYNC_RECONNECT_GRACE` | `30` | Seconds a dropped participant keeps their seat. |
| `FLICKSYNC_CONNECT_GRACE` | `60` | Seconds a participant who joined over HTTP has to open their WebSocket. |
| `FLICKSYNC_HOST_LEAVE_POLICY` | `transfer` | When the host leaves for good: `transfer` or `close`. |
| `FLICKSYNC_DEFAULT_CONTROL_MODE` | `everyone` | Who controls playback by default: `everyone` or `host_only`. |

### Sync

Drift thresholds in ms (0 < IGNORE < SOFT < HARD): under IGNORE nothing; under SOFT gentle speed
change; under HARD stronger speed change; HARD and over, a hard seek.

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_SYNC_DRIFT_IGNORE` | `100` | Drift threshold, ms. |
| `FLICKSYNC_SYNC_DRIFT_SOFT` | `500` | Drift threshold, ms. |
| `FLICKSYNC_SYNC_DRIFT_HARD` | `1500` | Drift threshold, ms. |
| `FLICKSYNC_SYNC_RATE_SOFT` | `0.03` | Speed change of the gentle band (0.03 = 3%). |
| `FLICKSYNC_SYNC_RATE_STRONG` | `0.08` | Speed change of the stronger band. |
| `FLICKSYNC_SYNC_SEEK_COOLDOWN_MS` | `3000` | Minimum ms between two hard seeks of one participant. |
| `FLICKSYNC_SYNC_HEARTBEAT` | `10` | Seconds between sync heartbeats while a room plays. |
| `FLICKSYNC_RATE_MIN` | `0.25` | Accepted playback speed range, lower bound. |
| `FLICKSYNC_RATE_MAX` | `4.0` | Accepted playback speed range, upper bound. |
| `FLICKSYNC_MAX_POSITION` | `604800` | Furthest seekable position (seconds). |

### Chat

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_CHAT_ENABLED` | `true` | Turn chat on or off. |
| `FLICKSYNC_CHAT_MAX_LENGTH` | `500` | Characters per message. |
| `FLICKSYNC_CHAT_HISTORY` | `100` | Messages kept per room. |
| `FLICKSYNC_CHAT_RATE_PER_SEC` | `1` | Per user: sustained messages per second. |
| `FLICKSYNC_CHAT_BURST` | `5` | Per user: burst. |

### Connections and abuse limits

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_MAX_CONNECTIONS` | `10000` | Simultaneous WebSocket connections. |
| `FLICKSYNC_WS_MAX_MESSAGE_BYTES` | `16384` | Largest inbound message (bytes). |
| `FLICKSYNC_WS_PING_INTERVAL` | `20` | Server ping interval (seconds). |
| `FLICKSYNC_WS_IDLE_TIMEOUT` | `60` | Silence after which a peer is dropped (seconds). |
| `FLICKSYNC_WS_SEND_TIMEOUT` | `10` | Seconds a send to a peer may take before it is dropped. |
| `FLICKSYNC_MSG_RATE_PER_SEC` | `20` | Per connection: messages per second. |
| `FLICKSYNC_MSG_BURST` | `40` | Per connection: burst. |
| `FLICKSYNC_WS_RATE_LIMIT_STRIKES` | `20` | Rate-limit violations in a row before the socket closes. |
| `FLICKSYNC_WS_OUTBOUND_BUFFER` | `256` | Outbound queue per connection. A connection that fills it is dropped as too slow. |

### Browser access (CORS)

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKSYNC_CORS_ORIGINS` | empty | Allowed browser origins, comma separated. Empty = none. Native clients are unaffected. `*` is refused. |

## FlickDD (Direct Download)

Docs: [flickdd-integration.md](flickdd-integration.md). Throttled, resumable downloads of media
files, proxied from Jellyfin / Plex. Unlike FlickSync, the file bytes pass through this server.

### Switch and backends

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKDD_ENABLED` | `false` | Master switch. When false, `/api/v1/downloads` answers 404 and every other `FLICKDD_*` variable is ignored (never validated), so a half-written FlickDD setup cannot stop FlickSync. |
| `FLICKDD_JELLYFIN_URL` | unset | Jellyfin base URL (reachable from this server), e.g. `http://jellyfin:8096`. |
| `FLICKDD_JELLYFIN_API_KEY` | unset | Jellyfin API key (Dashboard > API Keys). |
| `FLICKDD_PLEX_URL` | unset | Plex base URL, e.g. `http://plex:32400`. |
| `FLICKDD_PLEX_TOKEN` | unset | Plex `X-Plex-Token`. |

At least one backend is required when enabled. Each needs both its URL and its secret, or neither.

### Limits

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKDD_MAX_PARALLEL` | `10` | Downloads in progress per user (a slot is held from creation to completion or cancel). |
| `FLICKDD_MAX_GLOBAL` | `100` | Downloads in progress on the whole server. Must be >= `FLICKDD_MAX_PARALLEL`. |
| `FLICKDD_RATE_MBPS` | `10` | Speed cap per download, in MiB/s. Size your uplink as parallel downloads x this value. |
| `FLICKDD_CHUNK_MB` | `8` | Segment size clients should ask for with Range, in MiB (sent to clients as `chunk_bytes`). |
| `FLICKDD_MAX_RANGE_MB` | `64` | Largest answer to one ranged request, in MiB. Must be >= `FLICKDD_CHUNK_MB`. |
| `FLICKDD_MAX_REQUESTS_PER_MIN` | `120` | Requests per minute on one download (ranged resumes included). |
| `FLICKDD_MAX_OVERSERVE` | `2` | A download ends once it has served this many times the file size (guards against endless re-reads). |

### Lifetime

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKDD_GRANT_TTL` | `21600` | Seconds a download may stay idle before it expires (6 h). |
| `FLICKDD_GRANT_MAX_AGE` | `86400` | Seconds after which a download expires whatever its activity (24 h). |

### Timeouts and retries

| Variable | Default | Meaning |
| --- | --- | --- |
| `FLICKDD_STALL_TIMEOUT` | `30` | Seconds a slow client may block the stream before the response is cut (the client resumes). |
| `FLICKDD_UPSTREAM_TIMEOUT` | `15` | Seconds to wait for Jellyfin / Plex to answer or to send bytes. |
| `FLICKDD_UPSTREAM_RETRIES` | `2` | Times the server retries a failed upstream read inside one response before cutting it. |

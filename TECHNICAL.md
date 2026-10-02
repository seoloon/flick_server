# Flick Server: technical guide

Everything you need to run, configure, understand and develop Flick Server,
explained in plain words. Looking for the pitch? See the [README](README.md).

**Contents**
1. [What it is](#1-what-it-is)
2. [The big picture](#2-the-big-picture)
3. [Install](#3-install)
4. [The invitation link](#4-the-invitation-link)
5. [The web panel](#5-the-web-panel)
6. [Putting it on the internet](#6-putting-it-on-the-internet)
7. [Settings](#7-settings)
8. [How synchronization works](#8-how-synchronization-works)
9. [Security](#9-security)
10. [Monitoring and operations](#10-monitoring-and-operations)
11. [Development](#11-development)
12. [Where to read more](#12-where-to-read-more)

---

## 1. What it is

Flick Server is made of two parts:

| Part | What it does | Always on? |
|---|---|---|
| **FlickSync** | The Watch Together service. Keeps track of rooms and keeps everyone's playback in sync. | Yes |
| **Web panel** | A website for the person running the server: invitation, live rooms, statistics. | Optional |

FlickSync is **not** a media server. It never sees a video file and never talks
to Jellyfin or Plex. It only knows a *reference* to the title being watched
(think "episode 3 of that show"). Each Flick app then opens that title from its
own server.

## 2. The big picture

```
        ┌──────────┐   room, play, pause, seek, chat    ┌──────────────┐
        │ Flick app│ ◀────────────────────────────────▶ │              │
        │ (Alice)  │                                    │   FlickSync  │
        └──────────┘                                    │              │
        ┌──────────┐                                    │ rooms live   │
        │ Flick app│ ◀────────────────────────────────▶ │ in memory    │
        │  (Bob)   │                                    └──────▲───────┘
        └──────────┘                                           │ admin API
                                                        ┌──────┴───────┐
        Alice and Bob each stream the film from         │  Web panel   │
        their own Jellyfin or Plex.                     │  (optional)  │
                                                        └──────────────┘
```

Two kinds of conversation happen:
- **Plain web requests (REST)** for the simple things: create a room, join, leave.
- **A live connection (WebSocket)** for everything that must be instant: play,
  pause, seek, who just joined, chat, sync corrections.

Messages are small JSON texts. The full list is in [docs/protocol.md](docs/protocol.md).

**No database.** Rooms exist only in the memory of the running process. That
keeps Flick Server simple and fast, and it means a restart ends the rooms in
progress. The one thing saved on disk is the signing key (see
[the invitation link](#4-the-invitation-link)).

## 3. Install

### With Docker Compose (recommended)

```sh
cp .env.example .env
# edit .env: at least FLICKSYNC_PUBLIC_URL=https://your.domain
docker compose up -d --build
curl localhost:8787/health                      # the server answers
docker compose exec flicksync flicksync invite  # prints the invitation link
```

`docker-compose.yml` starts two services, `flicksync` and `panel`. The panel
exits straight away unless `ENABLE_WEB_PANEL=true`, so you can ignore it if you
do not want it. To start only FlickSync:

```sh
docker compose up -d flicksync
```

The compose file only *exposes* the ports to other containers and to your
reverse proxy. To reach a service directly from the host (for a quick local
test), add a `ports:` entry such as `"8787:8787"` to it.

The Dockerfile is a single file with two **targets**. Compose picks the right
one for each service, and you can build one yourself:

```sh
docker build --target flicksync -t flicksync .    # the sync service (also the default)
docker build --target panel     -t flick-panel .  # the panel
```

### Dokploy, Coolify and similar platforms

These tools build **one container per application**. Two ways to use them:

- **Compose service (one deployment, both containers).** Point it at
  `docker-compose.yml`, paste your `.env` in its environment tab, then add a
  domain for each service: `flicksync` on port `8787` and `panel` on port
  `3000`.
- **Two applications.** Same repository, one with build target `flicksync`
  and one with build target `panel`. The panel also needs `FLICKSYNC_URL` set
  to the internal address of the FlickSync application.

Without a build target, a platform builds the last stage of the Dockerfile,
which is FlickSync. That is why the panel is missing when only the plain
Dockerfile is deployed.

### Without Docker

You need Rust 1.88 or newer.

```sh
cargo run --release
```

## 4. The invitation link

Every FlickSync server has a secret **signing key**. The Flick app needs it to
prove it is allowed in. To save you from copying keys around, FlickSync wraps
everything into one link:

```
flicksync://your.domain/?v=1&tls=1#k=...
```

- On first start FlickSync **creates the key itself** and stores it in
  `FLICKSYNC_DATA_DIR` (the `/data` volume in Docker, file `auth_keys`).
- It prints the link in its logs, and you can show it again any time:
  ```sh
  docker compose exec flicksync flicksync invite
  docker compose exec flicksync flicksync invite --qr       # as a QR code
  docker compose exec flicksync flicksync invite --rotate   # new key (then restart)
  ```
- The link contains the secret. **Share it only with people you trust.**
  Rotating the key cancels every old link.
- Set `FLICKSYNC_PUBLIC_URL` to your public address, because behind a proxy
  FlickSync cannot guess it. Without it the link uses the local address over
  plain http.

If you prefer to manage keys yourself, `FLICKSYNC_AUTH_KEYS` (or
`FLICKSYNC_AUTH_KEYS_FILE`) replaces the automatic key. More in
[docs/flick-integration.md](docs/flick-integration.md) and
[docs/client-invitation.md](docs/client-invitation.md).

## 5. The web panel

A website for you, the person running the server. It shows:

- the **invitation link** (hidden until you reveal it, with a copy button and a QR code),
- the **live rooms**, with a button to close frozen ones,
- **statistics**: rooms, participants, latency, traffic, sync corrections and how far apart people drift.

To turn it on, add this to `.env` and redeploy:

```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>        # openssl rand -base64 18
FLICKSYNC_ADMIN_TOKEN=<16+ characters> # openssl rand -base64 32
```

`FLICKSYNC_ADMIN_TOKEN` is a shared secret between the panel and FlickSync, so
it must be the same on both. Without it FlickSync keeps its admin API switched
off. The panel refuses to start with a weak password or token.

It listens on port `3000`, or on the domain you gave it. Details and security
model: [panel/README.md](panel/README.md). The API behind it:
[docs/admin-api.md](docs/admin-api.md).

## 6. Putting it on the internet

Flick needs to reach FlickSync from outside, normally over **HTTPS**. Put a
reverse proxy (Traefik, Nginx, Caddy, Cloudflare) in front of it. The one thing
to check is that it lets **WebSockets** through, since that is how the live
connection works. Examples for each are in [docs/deployment.md](docs/deployment.md).

### One domain for the panel and FlickSync

You do not need two subdomains. Give each component its own path on the same
domain, for example:

| Address | Goes to |
|---|---|
| `https://flick.example.com/` | the panel |
| `https://flick.example.com/sync` | FlickSync (the proxy removes `/sync` before forwarding) |

Then set `FLICKSYNC_PUBLIC_URL=https://flick.example.com/sync` so the
invitation link carries the path. A ready Traefik setup is in
[docs/deployment.md](docs/deployment.md#one-domain-for-several-components-traefik).
In Dokploy, add two domains on the same host, the second with the `/sync` path
and *Strip Path* on.

## 7. Settings

Everything is configured with environment variables, read from `.env`. All are
optional. The complete, commented list is in [.env.example](.env.example); here
are the ones that matter most.

| Setting | Default | Meaning |
|---|---|---|
| `FLICKSYNC_PUBLIC_URL` | unset | Your public address. Set it behind a proxy. |
| `FLICKSYNC_PORT` | `8787` | Port FlickSync listens on. |
| `FLICKSYNC_DATA_DIR` | `/data` in Docker | Where the signing key is stored. Keep it on a volume. |
| `ENABLE_WEB_PANEL` | `false` | Turns the panel on. |
| `FLICKSYNC_LOG_LEVEL` | `info` | How chatty the logs are. |
| `FLICKSYNC_MAX_ROOM_SIZE` | `100` | People per room. |
| `FLICKSYNC_MAX_ROOMS` | `10000` | Rooms at once. |
| `FLICKSYNC_ROOM_CREATE_PER_MINUTE` | `6` | Rooms one person may create per minute. |
| `FLICKSYNC_HOST_LEAVE_POLICY` | `transfer` | When the host leaves for good: `transfer` the remote to someone, or `close` the room. |
| `FLICKSYNC_DEFAULT_CONTROL_MODE` | `everyone` | Who controls playback: `everyone` or `host_only`. |
| `FLICKSYNC_CHAT_ENABLED` | `true` | Chat on or off. |
| `FLICKSYNC_CORS_ORIGINS` | empty | Websites allowed to talk to the server from a browser. The Flick app does not need this. |
| `FLICKSYNC_METRICS_ENABLED` | `false` | Exposes `/metrics` for monitoring. |

Timers worth knowing:

| Setting | Default | Meaning |
|---|---|---|
| `FLICKSYNC_RECONNECT_GRACE` | 30 s | A dropped person keeps their seat this long. |
| `FLICKSYNC_ROOM_TIMEOUT` | 60 s | An empty room is deleted after this. |
| `FLICKSYNC_ROOM_IDLE_TIMEOUT` | 12 h | A room with no activity expires after this. |

## 8. How synchronization works

The goal: everyone sees the same frame at the same time, without jerky jumps.

1. **One shared truth.** The room holds a single *playback state*: a position,
   a speed, and the moment it was set. Each action (play, pause, seek, speed)
   updates it and gets a sequence number so nothing is applied twice or out of order.
2. **A heartbeat.** While the film plays, FlickSync sends the state every
   10 seconds (`FLICKSYNC_SYNC_HEARTBEAT`) so each app can compare itself to it.
3. **Gentle corrections.** Each app measures how far it is from where it should
   be, then reacts in proportion:

   | Drift | What happens |
   |---|---|
   | under 100 ms | nothing, nobody can tell |
   | 100 to 500 ms | very slight speed change (about 3%) |
   | 500 ms to 1.5 s | stronger speed change (about 8%) |
   | over 1.5 s | a real seek, at most once every 3 s per person |

   These limits are the `FLICKSYNC_SYNC_*` settings.
4. **Late arrivals.** Someone who joins mid-film receives the current state
   and catches up.

The logic is deliberately kept "pure": it takes the current time as an input
instead of reading a clock, so it can be tested without waiting.

## 9. Security

- **Signed tokens.** Every request carries a short-lived token (HS256 JWT)
  signed with the server key. Tokens are checked for signature, audience,
  expiry and the server they belong to. Too-long lifetimes are refused
  (`FLICKSYNC_AUTH_MAX_TOKEN_TTL`).
- **Key rotation.** Several keys can be active at once, so you can add a new
  one, switch over, then remove the old one without a cut.
- **Limits everywhere.** Rooms per minute, messages per second, message size,
  connections in total, and a cap on slow connections. Someone who keeps
  breaking the rules is disconnected.
- **Chat is cleaned** before it is stored or forwarded.
- **Browsers are locked out by default.** CORS allows no website unless you list it.
- **Hardened containers.** Read-only filesystem, all Linux privileges dropped,
  non-root user, and a FlickSync image with no shell at all.
- **The panel** needs a password and uses a signed `HttpOnly` session cookie.
  The admin token stays on the panel's server side and never reaches the browser.
- **Treat the invitation link like a password.**

## 10. Monitoring and operations

| Address | Purpose |
|---|---|
| `/health` | Is the process alive? |
| `/ready` | Is it ready to take traffic? |
| `/metrics` | Prometheus statistics. Off by default. Set `FLICKSYNC_METRICS_ENABLED=true`, and optionally `FLICKSYNC_METRICS_TOKEN` to require a bearer token. |

- **Logs** go to the console. Use `FLICKSYNC_LOG_FORMAT=json` for log tools.
- **Docker health checks** are built in. The image has no `curl`, so the
  program checks itself with `flicksync healthcheck`.
- **Shutting down** gives open connections `FLICKSYNC_SHUTDOWN_GRACE` seconds
  (10 by default) to finish.
- **Backups:** only the `/data` volume matters, as it holds the signing key.
  Without it, issued invitation links stop working.

## 11. Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test        # unit, integration (real WebSockets), protocol and concurrency tests
```

Needs Rust 1.88 or newer. The panel has its own tests: `cd panel && npm test`.

To try the server without the Flick app, mint a token by hand with the example
program. The key and its format (`kid:server_id:secret`) are in
`$FLICKSYNC_DATA_DIR/auth_keys`:

```sh
TOKEN=$(cargo run -q --example mint_token -- main <secret> my-flick alice Alice)
curl -s -X POST localhost:8787/api/v1/rooms -H "Authorization: Bearer $TOKEN"
```

### Layout

```
src/
  api/         web requests: rooms, health, admin
  websocket/   the live connection
  protocol/    message formats and validation
  room/        the room: who is in, who may do what
  sync/        playback state and drift rules
  chat/        chat cleaning and history
  auth/        token checking and keys
  config/      settings read from the environment
panel/         the Next.js web panel
tests/         integration tests with real sockets
docs/          deeper references
```

The rules of a room and the sync maths are kept apart from networking, so they
are easy to test and reason about.

## 12. Where to read more

| Document | For |
|---|---|
| [docs/protocol.md](docs/protocol.md) | Every message, payload and error. For client developers. |
| [docs/flick-integration.md](docs/flick-integration.md) | What the Flick app and server must do; tokens and key rotation. |
| [docs/client-invitation.md](docs/client-invitation.md) | The invitation link format. |
| [docs/deployment.md](docs/deployment.md) | Docker, Dokploy, Coolify, bare metal, proxy setups. |
| [docs/admin-api.md](docs/admin-api.md) | The operator API behind the panel. |
| [docs/architecture.md](docs/architecture.md) | Design choices and risks. |
| [docs/scaling.md](docs/scaling.md) | What running several instances would require. |
| [docs/openapi.yaml](docs/openapi.yaml) | The REST API (OpenAPI 3). |
| [.env.example](.env.example) | Every setting, commented. |

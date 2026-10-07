# Module lifecycle and panel-managed settings: design

First of four sub-projects that make the Flick Panel the place where Flick Server is run:

1. **Server core (this spec)**: stored settings, admin API, module start / stop / reload.
2. Panel: a settings page per module and for the server, enable switches.
3. Logs: in-memory log buffer on the server and a live log view in the panel.
4. Environment and compose: a minimal `.env`, panel on by default.

Sub-projects 2 to 4 get their own spec. This one defines everything they need from the server.

## 1. Goal and constraints

- The `flick-modules` container always starts. With a fresh install **no module is running**:
  only `/health`, `/ready` and the admin API answer. A module runs only once someone enabled it.
- Each module (FlickSync, FlickDD) can be **started, stopped and reloaded** at runtime, without
  restarting the container and without touching the other module.
- Settings move from environment variables to a file edited through the admin API. The
  environment stays a fallback, so existing deployments keep working.
- Lightest, most stable, most secure, most future-proof. In practice: little change to the
  existing code, one fixed router, no Docker socket, and a module contract that a third module
  can implement.

Non-goals: the panel UI, the log viewer, the new `.env.example` (sub-projects 2 to 4); changing
the client API or the invitation format; persisting rooms or downloads (still in memory).

## 2. Decisions taken

| Question | Decision |
|---|---|
| How a setting takes effect | Enable / disable is immediate. Other settings are saved at once and applied when the module is **reloaded** (stop then start), never by restarting the container. |
| Shared settings | A "server" scope (public address, JWT keys / audience / TTL / leeway, CORS, metrics, log level), edited on its own page. |
| Secrets in `.env` | Only `PANEL_PASSWORD`. The admin token is derived from it. `FLICKSYNC_ADMIN_TOKEN` is removed. |
| Environment vs panel | Stored setting > environment variable > code default. The environment keeps working as a default. |
| Architecture | Fixed router, one swappable slot per module (approach A). |

## 3. Architecture

### 3.1 Modules

```rust
trait Module {
    const ID: &'static str;                       // "flicksync" | "flickdd"
    fn start(settings: &Settings) -> Result<Running, ModuleError>;
    fn stop(running: Running);                    // close rooms / streams, free memory
}
```

`ModuleSlot<M>` holds `RwLock<ModuleState<M>>` with

```
Stopped | Running(Arc<M::Running>, applied_revision) | Failed(message)
```

- `start` on a running module and `stop` on a stopped one are no-ops (idempotent).
- A `Mutex` per module serialises `start`, `stop` and `reload`, so two clicks cannot interleave.
- `reload` = `stop` then `start` with the settings stored at that moment. If `start` fails the
  module ends in `Failed(message)`, **not** running; the message is shown in the panel.
- A failing `start` (for example FlickDD enabled without any backend) never stops the process.
  Today an invalid `FLICKDD_*` aborts the whole boot; that behaviour disappears for module
  settings. Invalid **boot** variables (`FLICKSYNC_HOST`, `FLICKSYNC_PORT`, ...) still abort.

FlickSync's running state is `RoomManager` + `ConnLimit` + the sync / chat / room configs it
reads. FlickDD's is the existing `DdState`. Request handlers load the slot, clone the `Arc`, and
use it; a reload cannot free memory under a request.

### 3.2 Routes stay mounted

The router is built once. Each module route starts by loading its slot:

| Module stopped | Answer |
|---|---|
| FlickSync routes (`/api/v1/rooms/**`, WebSocket upgrade) | `503` `MODULE_DISABLED` (JSON error shape) |
| FlickDD routes (`/api/v1/downloads/**`) | `404`, as documented today |
| Admin `dd/*` GET routes | `404` as today (panel shows "FlickDD is off") |

`/health` is unaffected (container probe). `/ready` means "the server can serve": signing keys
loaded. It no longer depends on the room manager. `/metrics` stays server-level; module
counters are zero while the module is stopped.

`stop` of FlickSync closes every room and WebSocket (same path as the current graceful
shutdown); `stop` of FlickDD cuts every stream (`DdState::shutdown`). Clients reconnect or
resume on their own.

### 3.3 Server scope

`state.cfg` is a fixed `Arc<Config>` read in many places. It becomes a **runtime config**
behind `RwLock<Arc<ServerRuntime>>`, covering what the server scope edits: public endpoint,
`Authenticator`, allowed origins, metrics switch / token, body limit. Reading code calls
`state.server()` to get the current `Arc`. Consequences:

- CORS: the `CorsLayer` built at startup is replaced by a small layer reading the current
  allowed origins per request.
- Log level: `tracing_subscriber::reload` handle; `FLICKSYNC_LOG_FORMAT` stays boot-only.
- Reloading the server scope swaps the `Authenticator` and the runtime values. Running modules
  are **not** restarted; tokens already issued stay valid as long as their key stays in the set.
- Everything bound at start (`FLICKSYNC_HOST`, `FLICKSYNC_PORT`, `FLICKSYNC_DATA_DIR`) is
  boot-only.

No new dependency: `std::sync::RwLock` is enough (writes are rare, reads are a clone of an `Arc`).

## 4. Settings

### 4.1 File

`<DATA_DIR>/settings.json`, owned by the server user, mode `0600`, written atomically
(temp file, `fsync`, rename). The data directory is already a persistent volume.

```json
{
  "version": 1,
  "revision": 17,
  "server":    { "public_url": "https://flick.example.com/services", "log_level": "info" },
  "flicksync": { "enabled": true, "max_room_size": 100 },
  "flickdd":   { "enabled": false, "jellyfin": { "url": "...", "api_key": "..." } }
}
```

`revision` increases on every write; a running module remembers the revision it was started
with, so the API can say "this module runs older settings than stored" (`pending_reload`).
`version` is the schema version for future migrations. Unknown keys are kept on rewrite and
ignored on read.

### 4.2 Precedence and validation

The existing parsing is built on a lookup function (`Config::from_lookup`, `DdConfig::from_lookup`).
The stored settings are exposed through the **same function**, layered in front of the process
environment: lookup(name) = stored value (under the variable name it replaces) else environment.
All current validation (drift thresholds, chunk <= range, "backend needs URL and secret", ...) is
reused unchanged, and the defaults stay in the code.

Each value reported by the API carries its source: `panel`, `environment` or `default`.
`enabled` has no default other than `false`; `FLICKDD_ENABLED=true` or a new
`FLICKSYNC_ENABLED=true` in the environment are honoured as the fallback, so an existing
`.env` keeps its modules. **Upgrade note:** an existing install with neither variable comes
back with FlickSync stopped until it is enabled once in the panel.

### 4.3 What leaves the environment

| Stays in the environment (boot / bootstrap) | Moves to settings |
|---|---|
| `FLICKSYNC_HOST`, `FLICKSYNC_PORT`, `FLICKSYNC_DATA_DIR`, `FLICKSYNC_LOG_FORMAT` | **server:** `PUBLIC_URL`, `AUTH_KEYS`, `AUTH_AUDIENCE`, `AUTH_MAX_TOKEN_TTL`, `AUTH_LEEWAY`, `CORS_ORIGINS`, `METRICS_ENABLED`, `METRICS_TOKEN`, `LOG_LEVEL` |
| `FLICKSYNC_AUTH_KEYS_FILE` (secret mounts), `FLICKSYNC_SHUTDOWN_GRACE`, `FLICKSYNC_SWEEP_INTERVAL_MS` | **flicksync:** `enabled`, rooms, sync, chat, connection and abuse limits (everything under sections "Rooms" to "Connections" of today's `.env.example`) |
| `PANEL_PASSWORD`, and for the panel container `ENABLE_WEB_PANEL`, `FLICKSYNC_URL`, `PORT` | **flickdd:** `enabled`, backends (URL + secret), limits, lifetime, timeouts |

Removed: `FLICKSYNC_ADMIN_TOKEN`. While the panel is not yet updated (section 9) the server still
accepts it, logs a deprecation warning, and accepts both it and the derived token. The server
container already receives `PANEL_PASSWORD` through the same `env_file` as the panel.

### 4.4 Secrets

Secrets are the Jellyfin API key, the Plex token, the metrics token and the JWT keys
(`AUTH_KEYS`). They are **write-only through the API**: a read returns `{"set": true}`, never the
value. On write, an omitted secret keeps its stored value, a string replaces it, `null` clears it.
They are never logged (extends the existing `Debug` redaction). The file is `0600`.

## 5. Admin API

All routes are under `/admin/v1`, behind the admin bearer token, every answer `no-store`
(existing layer), in the existing error shape.

| Route | Purpose |
|---|---|
| `GET /modules` | For each module: `state` (`stopped` / `running` / `failed`), `message` when failed, `enabled`, `pending_reload`, `since` |
| `POST /modules/{id}/start` | Persist `enabled=true`, then start. Failure: state `failed` with the message, answer `200` with the module status |
| `POST /modules/{id}/stop` | Persist `enabled=false`, then stop |
| `POST /modules/{id}/reload` | Stop then start with stored settings; keeps `enabled` |
| `GET /settings/{server\|flicksync\|flickdd}` | Values with source, default, constraints, secrets masked |
| `PUT /settings/{scope}` | Validate the **whole** resulting scope, write on success. Invalid: `400` with the field and the reason, nothing written |
| `POST /settings/server/reload` | Apply the server scope (section 3.3) |

`enabled` is persisted by `start` / `stop`, so it survives a container restart: at boot the
server starts the modules whose `enabled` is true.

Unknown module id: `404`. Settings for a field that does not exist: `400`.

**Admin token.** `token = hex(HMAC-SHA256(key = PANEL_PASSWORD, msg = "flick-admin-api-v1"))`,
computed by the server at boot and by the panel (Node `crypto`). The admin API answers `404`
unless `PANEL_PASSWORD` is at least 10 characters (same rule as the panel). Comparison is
constant-time. The `hmac` crate is added next to `sha2`.

## 6. Error handling

- Invalid settings never reach the file and never break a running module.
- A module in `Failed` keeps its slot empty; handlers answer as if stopped.
- A crash inside `stop` (panic in a task) is contained: the slot is still emptied and the error
  is logged.
- Disk errors on write: `500` with a generic message, the previous file untouched (atomic rename).
- A corrupt `settings.json` at boot: the server refuses to start and names the file. It is never
  silently replaced by defaults (same stance as the signing key file).

## 7. Security

- No Docker socket anywhere; nothing in this sub-project gives a container more rights.
- `/admin` must not be routed publicly by the reverse proxy; the docs say so. The admin token is
  password-equivalent for the panel's powers but does not reveal the password.
- Stopped modules hold no connection, room, grant or backend credential in memory.
- Settings that open a network target (Jellyfin / Plex URL) keep today's restrictions: no
  redirects, secrets in headers only, errors never carry the URL.

## 8. Testing

- Layered lookup: stored > environment > default, with the reported source, and the same
  validation errors as today for each rule.
- Lifecycle: start / stop / reload; idempotence; `reload` picks up a changed value; a failing
  start leaves `Failed` and a working server; the other module is untouched.
- `stop` closes open rooms and WebSockets (FlickSync) and cuts a running download (FlickDD).
- Routes while stopped answer `503 MODULE_DISABLED` / `404`, `/health` stays 200, `/ready`
  follows the keys.
- Admin API: secrets never appear in any response (extends the existing "grant tokens never
  appear" test); invalid `PUT` writes nothing; omitted secret keeps its value; `null` clears it.
- File: atomic write (no partial file after a simulated failure), `0600`, corrupt file refused.
- Token derivation matches a fixed vector on both sides (Rust and Node) so the two stay in sync.

## 9. Rollout

One change set per concern, each leaving the tests green: runtime config + `ServerRuntime`;
settings file + layered lookup; module slots + lifecycle; admin routes; token derivation and
removal of `FLICKSYNC_ADMIN_TOKEN` (panel updated in sub-project 2, so the old variable is
accepted with a deprecation warning until then).

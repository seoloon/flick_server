# Minimal environment, panel on by default, README: design

Fourth and last sub-project that makes the Flick Panel the place where Flick Server is run (list in
`2026-10-07-module-lifecycle-settings-design.md`). Sub-projects 1 to 3 (server core, panel settings,
logs) are done; this one removes what they made obsolete and brings the documentation in line.

## 1. Goal and constraints

- A new install needs a handful of lines in `.env`. Everything else is set in the panel
  (stored in `settings.json`); the environment stays a fallback, so existing installs keep working.
- The panel container is on by default in `docker-compose.yml`. FlickSync stays off until someone
  enables it (panel switch, or `FLICKSYNC_ENABLED=true`): unchanged.
- One secret for the panel and the admin API: `PANEL_PASSWORD`. `FLICKSYNC_ADMIN_TOKEN` is removed.
- The README tells what Flick Server now is: a **utility server that adds features to Flick**, run
  from a panel. Watch Together (FlickSync) and Direct Download (FlickDD) are its first two modules,
  not its definition. It uses the new FlickServer wordmarks.
- No behaviour change for the Flick clients, the client API or the invitation format.

Non-goals: new settings, a new module, changing `settings.json`, changing how modules start.

## 2. Decisions

| Question | Decision |
|---|---|
| What stays in `.env.example` | Host and port, `FLICKSYNC_DATA_DIR`, `FLICKSYNC_PUBLIC_URL`, `FLICKSYNC_LOG_LEVEL`, `FLICKSYNC_LOG_FORMAT`, `FLICKSYNC_LOG_BUFFER`, `PANEL_PASSWORD`, `ENABLE_WEB_PANEL`, `FLICKSYNC_AUTH_KEYS` / `_FILE`, `FLICKSYNC_SHUTDOWN_GRACE`, `FLICKSYNC_SWEEP_INTERVAL_MS`, `FLICKSYNC_ENABLED`. These are boot-only, secrets, or what a first start needs. |
| Where the rest of the variables go | A new `docs/configuration.md`: the full reference of every variable (the comments of today's `.env.example`, grouped by module), stated to be the fallback behind the panel. `.env.example` and `TECHNICAL.md` link to it. Nothing is lost. |
| `FLICKSYNC_ADMIN_TOKEN` | Removed from server config, `AdminTokens`, the boot-only list, the start-up deprecation warning, panel token resolution, panel env check, error texts and docs. A value left in an old `.env` is ignored. The server's existing line "admin API is off: set PANEL_PASSWORD" covers an install that only had the legacy token. |
| Panel default in compose | `environment: ENABLE_WEB_PANEL: ${ENABLE_WEB_PANEL:-true}` on the `panel` service (the `environment` entry wins over `env_file`; `.env` or the shell can still set `false`). With no valid `PANEL_PASSWORD` the panel exits with its existing explicit message and FlickSync is unaffected. |
| Restart policy of `panel` | `on-failure:5`, so a missing password does not loop forever. A deliberate `ENABLE_WEB_PANEL=false` exits 0 and is not restarted, as today. |
| Panel reachability | Unchanged: `expose` only (reverse proxy or a `ports:` line). Documented, not changed. |
| Wording "only the signing key is on disk" | Now: the signing key and `settings.json`, the two files the service writes (`TECHNICAL.md:70,369`, `Dockerfile:59`, `docs/deployment.md:62`). |

## 3. README

- Header: `flickserver-wordmark` assets in the dark/light `<picture>`. The implementer inspects the
  fills of `docs/assets/flickserver-wordmark.svg`, `-flat.svg` and `-flat-dark.svg` and picks the
  variant for each scheme; the old `flick-wordmark-*.svg` references go.
- Tagline and "Why Flick Server": a companion utility for Flick that adds features the player cannot
  have alone, run from one panel. Modules today: **Watch Together** and **Direct Download**; the
  design lets more be added. Light, private, set up in minutes stays.
- Highlights: add **modules you switch on and off from the panel** (start, stop, reload without a
  restart), **settings in the panel** (no `.env` editing after the first start), **live logs** (in
  memory, secrets masked). Existing items stay: sync, invitation link, downloads, chat, resilience,
  tiny footprint.
- "Private by design" and "Get started" updated: only `PANEL_PASSWORD` and `FLICKSYNC_PUBLIC_URL`
  are needed; the panel is on by default; FlickSync is enabled from the panel or with
  `FLICKSYNC_ENABLED=true`. The old "Want the panel?" block goes.
- Same voice as today (plain, short, no jargon). `TECHNICAL.md` stays the detailed reference.

## 4. Testing

- Server: `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`. Tests that started a
  server with `FLICKSYNC_ADMIN_TOKEN` now start it with `PANEL_PASSWORD` and use the derived token;
  `config` tests drop the legacy cases; one test shows a set `FLICKSYNC_ADMIN_TOKEN` is ignored.
- Panel (`panel/`): `npm test`, `npm run typecheck`, `npm run build`; legacy cases removed from
  `admin-token`, `env-check` and `lib` tests.
- Compose: `docker compose config` (if Docker is available) shows `ENABLE_WEB_PANEL=true` for the
  panel with an empty `.env`; `grep -r FLICKSYNC_ADMIN_TOKEN` finds only the historical specs/plans
  and the one "ignored" test and upgrade note.
- Docs: every relative link in the files touched resolves.

## 5. Upgrade note

An install with `FLICKSYNC_ADMIN_TOKEN` and `PANEL_PASSWORD`: nothing to do, the line is ignored.
An install with only the legacy token had no panel password: set `PANEL_PASSWORD` (10+ characters).
Both go in `docs/deployment.md#upgrading-to-panel-managed-settings`.

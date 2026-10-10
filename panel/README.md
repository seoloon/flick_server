# Flick Panel

The web panel for Flick Server: one sidebar entry per component, **FlickSync** first. Next.js (App Router),
no database, no extra runtime dependency, styled with the Flick design system (tokens, glass, pills, Inter).

Overview: the server's state, the invitation link, and one card per module (FlickSync, FlickDD) with its state
(running, stopped, or failed to start with the reason), a "Reload required" badge when saved settings are not yet in
force, and Start / Stop / Reload. Stop and Reload ask first: they close rooms or cut downloads. A module stays as you
leave it, also after a server restart.

Settings (sidebar, or the Settings button of a module): one page per scope (Server, FlickSync, FlickDD), built from the
list of settings the server returns. Each setting shows where its value comes from (saved in the panel, the
environment, or the default); a value saved here wins over the environment and Reset removes it. Secrets are never
shown: replace or clear them. Save sends only what changed, shows the server's explanation when a value is refused,
then offers to apply the server settings or reload the module.

FlickDD page (once FlickDD runs with a Jellyfin or Plex backend set in its settings): downloads in progress with a stop button, recent downloads, bytes per day, top titles and the limits in force.

FlickSync page: copy the invitation link (masked until revealed, QR code on demand), list live rooms and close frozen
ones, see rooms, participants, latency, traffic, sync corrections and the distribution of measured drift.

Logs (sidebar): the server's latest lines, live (every 2 seconds). Filter by minimum level and by text, pause and
resume, copy or download the lines shown, clear the view (the server keeps its lines). The server keeps only its last
`FLICKSYNC_LOG_BUFFER` lines (2000 by default) in memory, so they are gone after a restart: use `docker compose logs`
for anything older. Values that look like secrets (tokens, passwords, keys) show as `<redacted>`. When the server does
not answer, the page keeps the lines it has, says so, and resumes by itself.

## Turn it on

In Docker Compose the panel is on by default. Set these in the repository's `.env` (see `.env.example`; every
variable is in [../docs/configuration.md](../docs/configuration.md)):

```
PANEL_PASSWORD=<10+ characters>          # sign-in to the panel, and the source of the admin token
# FLICKSYNC_PUBLIC_URL=https://sync.example.com   # so the invitation carries your real address
# ENABLE_WEB_PANEL=false                          # turns the panel off
```

Generate the password with `openssl rand -base64 18`.

The panel talks to Flick Server's [admin API](../docs/admin-api.md) with a token derived from the password:
`hex(HMAC-SHA256(PANEL_PASSWORD, "flick-admin-api-v1"))`, computed on the trimmed password. The server computes the
same token from the same `.env`, so there is no second secret to share.

* `ENABLE_WEB_PANEL=false`: the process logs one line and exits `0`. Nothing listens. (Compose sets `true` unless `.env` says otherwise; run standalone, the default is `false`.)
* `true`: the panel serves on port **3000** (`PORT` changes it; compose only exposes it to the other containers and the reverse proxy). It refuses to start without a password of at least 10 characters (leading and trailing spaces do not count).

### Docker Compose

```bash
docker compose up -d --build        # flicksync + panel
docker compose up -d flick-modules      # FlickSync only
```

The `panel` service reaches FlickSync at `http://flick-modules:8787` on the compose network and reads the same `.env`.

### From a checkout

```bash
cd panel
npm install
npm run build
ENABLE_WEB_PANEL=true PANEL_PASSWORD=... FLICKSYNC_URL=http://localhost:8787 npm start
```

Development: `npm run dev` (port 3000). Checks: `npm run typecheck`, `npm test`.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `ENABLE_WEB_PANEL` | `false` (`true` in compose) | Master switch |
| `PANEL_PASSWORD` | none | Password of the single panel account; the admin token is derived from it |
| `FLICKSYNC_URL` | `http://localhost:8787` | Where the panel reaches FlickSync (set by compose) |
| `PORT` | `3000` | Listening port inside the process |

## Security model

* The panel can read the invitation (the signing key) and close rooms, so put it behind HTTPS (a reverse proxy)
  and do not expose it more widely than needed. Behind a proxy, forward `X-Forwarded-Proto` so cookies are marked
  `Secure`, and `X-Forwarded-For` so the sign-in throttle sees real client addresses.
* Sign-in: constant-time password check, 5 failures per client and 15 minutes, then a lockout.
* Session: a signed, `HttpOnly`, `SameSite=Strict` cookie valid for 12 hours, with no server-side store. The key
  is derived from the password, so changing the password signs everyone out.
* The admin token and secret settings never reach the browser. The browser calls the panel's own `/api/*` routes,
  which need the session; those call the server's admin API. Every change (closing a room, cutting a download,
  starting or stopping a module, saving or applying settings) also checks that the request is same-origin.
* Strict CSP, `frame-ancestors 'none'`, no caching of pages or API responses.

## Adding a component

The panel is a base for the other Flick Server components. To add one:

1. Create `src/app/(panel)/<id>/page.tsx` (the layout already requires a session).
2. Add an entry to `src/lib/modules.ts` (with its `settingsHref`); the sidebar, the Overview card and the settings
   tabs pick it up. The server must know the module id and its settings scope (see `docs/admin-api.md`); add the id
   to `MODULE_IDS` in `src/lib/module-status.ts` and the scope to `SETTINGS_SCOPES` in `src/lib/settings-form.ts`.
3. Build the page from `src/components/flick/ui.tsx` (Button, Panel, Pill, Notice, Facts, Dialog, Segmented, ...).
   Follow the design system's content rules: Title Case for navigation and buttons, sentence case elsewhere,
   British spelling, no emoji, no exclamation marks, show only what the data says.
4. Reach the component's backend from a server route (see `src/lib/flicksync.ts` and `src/app/api/flicksync`),
   never from the browser with a secret.

## Design system

`scripts/flick-tokens.json` is the Flick design system's `tokens.json`; `npm run tokens` (also part of `npm run build`)
turns it into `src/styles/tokens.css`. `src/styles/flick-components.css` is the design system's component stylesheet,
unchanged; `panel.css` holds the panel's own layout and charts, using tokens only. Dark only, monochrome: no accent
colour is introduced, charts are told apart by weight and dash pattern.

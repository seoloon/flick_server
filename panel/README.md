# Flick Panel

The web panel for Flick Server: one sidebar entry per component, **FlickSync** first. Next.js (App Router),
no database, no extra runtime dependency, styled with the Flick design system (tokens, glass, pills, Inter).

FlickDD page (once FlickDD is enabled on the server, see `FLICKDD_*` in `.env.example`): downloads in progress with a stop button, recent downloads, bytes per day, top titles and the limits in force.

FlickSync page: copy the invitation link (masked until revealed, QR code on demand), list live rooms and close frozen
ones, see rooms, participants, latency, traffic, sync corrections and the distribution of measured drift.

## Turn it on

Both settings live in the repository's `.env` (see `.env.example`):

```
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>          # sign-in to the panel, and the source of the admin token
# FLICKSYNC_PUBLIC_URL=https://sync.example.com   # so the invitation carries your real address
```

Generate the password with `openssl rand -base64 18`.

The panel talks to Flick Server's [admin API](../docs/admin-api.md) with a token derived from the password:
`hex(HMAC-SHA256(PANEL_PASSWORD, "flick-admin-api-v1"))`, computed on the trimmed password. The server computes the
same token from the same `.env`, so there is no second secret to share. `FLICKSYNC_ADMIN_TOKEN` is deprecated: while it
is set the panel sends it instead (the server still accepts it) and logs a note at start; remove it to use the derived
token.

* `ENABLE_WEB_PANEL=false` (the default): the process logs one line and exits `0`. Nothing listens.
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
| `ENABLE_WEB_PANEL` | `false` | Master switch |
| `PANEL_PASSWORD` | none | Password of the single panel account; the admin token is derived from it |
| `FLICKSYNC_ADMIN_TOKEN` | none | Deprecated. When set, sent instead of the derived token; it must then equal the server's value (16+ characters) |
| `FLICKSYNC_URL` | `http://localhost:8787` | Where the panel reaches FlickSync (set by compose) |
| `PORT` | `3000` | Listening port inside the process |

## Security model

* The panel can read the invitation (the signing key) and close rooms, so put it behind HTTPS (a reverse proxy)
  and do not expose it more widely than needed. Behind a proxy, forward `X-Forwarded-Proto` so cookies are marked
  `Secure`, and `X-Forwarded-For` so the sign-in throttle sees real client addresses.
* Sign-in: constant-time password check, 5 failures per client and 15 minutes, then a lockout.
* Session: a signed, `HttpOnly`, `SameSite=Strict` cookie valid for 12 hours, with no server-side store. The key
  is derived from the password, so changing the password signs everyone out.
* The admin token never reaches the browser. The browser calls the panel's own `/api/flicksync/*` routes, which need
  the session; those call FlickSync server-side. Closing a room also checks that the request is same-origin.
* Strict CSP, `frame-ancestors 'none'`, no caching of pages or API responses.

## Adding a component

The panel is a base for the other Flick Server components. To add one:

1. Create `src/app/(panel)/<id>/page.tsx` (the layout already requires a session).
2. Add an entry to `src/lib/modules.ts`; the sidebar and Overview pick it up.
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

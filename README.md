<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/flickserver-wordmark-flat.svg">
    <img src="docs/assets/flickserver-wordmark-flat-dark.svg" alt="Flick Server" width="360">
  </picture>
</p>

<h3 align="center">Flick Server: the toolbox behind Flick.</h3>

<p align="center">
  Add features to Flick from your own server, one module at a time.<br>
  Light to run, private by design, set up in minutes.
</p>

<p align="center">
  <img alt="Docker" src="https://img.shields.io/badge/Docker-ready-2ea043?style=flat-square&logo=docker&logoColor=white">
  <img alt="Built with Rust" src="https://img.shields.io/badge/Rust-fast%20%26%20tiny-24c8db?style=flat-square&logo=rust&logoColor=white">
  <img alt="No database" src="https://img.shields.io/badge/database-none-8957e5?style=flat-square">
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square"></a>
</p>

---

## Why Flick Server

Flick Server is a small utility that runs next to your media server and gives
Flick features a player cannot have alone. It is built from **modules** you
switch on and off from a panel. Two exist today:

- **Watch Together**: everyone presses play at the same moment and stays on
  the same frame.
- **Direct Download**: save a film or an episode from Jellyfin or Plex to
  watch offline.

More can be added. A module is a self-contained feature with its own settings.

Flick Server does not host films and never touches your Jellyfin or Plex
library on its own. Watch Together moves no media, and Direct Download only
passes files through when you enable it. Each person keeps streaming from
their own library.

## Highlights

### 🖥️ A panel that runs everything
Modules start, stop and reload without restarting the container. Every setting
lives in the panel, and the logs are live, with secrets masked. You also get
the invitation link, the live rooms, sync statistics and current downloads, in
the same design as Flick.

### 🎬 Everyone on the same frame (Watch Together)
Play, pause, seek and speed are shared with the whole room. Small drifts are
corrected **smoothly** with a barely noticeable speed change. A real seek only
happens when someone is far behind.

### 🔗 One link to invite everyone
No account to create, no key to type. On first start Flick Server makes its
own key and gives you a **single invitation link**. Paste it into Flick and you
are connected. A QR code is one command away.

### 📥 Downloads for offline viewing (Direct Download)
Turn on the module and the Flick app can save a film or an episode from your
Jellyfin or Plex to watch offline. Downloads **resume** after any interruption,
and each one is speed-limited so nobody hogs your connection. Off by default.

### 💬 Chat in the room
Short messages next to the film, with history for people who join late and
limits that keep the room calm.

### 🛡️ Built to be left alone
- **Reconnects gracefully.** A dropped connection keeps its seat for a while.
- **Hands the remote over.** When the host leaves, someone else can take over.
- **Protects itself.** Rate limits, size limits and connection caps.
- **Tidies up.** Empty and idle rooms disappear on their own.

### 🪶 Tiny footprint
One process, written in Rust. Everything lives in memory: **no database, no
Redis, nothing to maintain**. A small VPS or a Raspberry Pi is plenty.

## Private by design

- **No telemetry.** Flick Server talks to your Flick clients and nobody else.
- **No media passes through it for Watch Together.** Only room state and a
  reference to the title. (If you turn on Direct Download, downloaded files do
  travel through the server on their way from your Jellyfin or Plex.)
- **Nothing is stored.** Rooms live in memory and vanish on restart. The only
  files written are the signing key and the settings you change from the panel
  (`settings.json`).
- **Short-lived signed tokens.** Access is checked on every request.
- **Panel locked behind a password.** On by default, but only reachable from
  your own network or reverse proxy: nothing is published to the internet by
  the compose file.

## Get started

You need Docker and a domain name pointing to your server.

```sh
cp .env.example .env     # set PANEL_PASSWORD (10+ characters) and FLICKSYNC_PUBLIC_URL=https://your.domain
docker compose up -d --build
```

Open the panel (port 3000 behind your reverse proxy, or add
`ports: ["3000:3000"]` for a quick local test), sign in, switch **Watch
Together** on, and copy the invitation link into Flick. Prefer the terminal?
Set `FLICKSYNC_ENABLED=true` in `.env` and run
`docker compose exec flick-modules flicksync invite`.

Upgrading an existing install? See [Upgrading to panel-managed settings](docs/deployment.md#upgrading-to-panel-managed-settings).

Using Dokploy, Coolify, a reverse proxy or no Docker at all? See
**[TECHNICAL.md](TECHNICAL.md)** and the [deployment guide](docs/deployment.md).

## Honest limits

- **One server, one process.** Rooms are not shared between several
  instances. That is plenty for a household and a large circle of friends.
- **Rooms do not survive a restart.** Restarting Flick Server ends the rooms
  in progress, and friends simply create a new one.
- **Everyone needs access to the title.** Flick Server does not stream
  anything: each person plays it from their own Jellyfin or Plex.
- **Downloads use your bandwidth.** With Direct Download on, size your upload speed for
  the downloads you allow (see [TECHNICAL.md](TECHNICAL.md#9-flickdd-downloads)).

## Documentation

How it works, security, deployment and development notes are in
**[TECHNICAL.md](TECHNICAL.md)**. Every variable is listed in
[docs/configuration.md](docs/configuration.md). Deeper references live in [`docs/`](docs).

## Built with

[Rust](https://www.rust-lang.org) · [Tokio](https://tokio.rs) ·
[Axum](https://github.com/tokio-rs/axum) · [Next.js](https://nextjs.org) ·
[Docker](https://www.docker.com)

## License

Flick Server is free software under the [GNU General Public License v3.0](LICENSE).

Flick is an independent project. It is not affiliated with Jellyfin or Plex.

## Notes from the dev

Built with Claude, fully brainstormed and thought by a human

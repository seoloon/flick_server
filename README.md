<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/flick-wordmark-dark.svg">
    <img src="docs/assets/flick-wordmark-light.svg" alt="Flick" width="360">
  </picture>
</p>

<h3 align="center">Flick Server: the companion server for Flick.</h3>

<p align="center">
  Watch together, in perfect sync, from your own server.<br>
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

Movie night does not stop when your friends live somewhere else. Flick Server
is the small service that makes **Watch Together** work in the Flick player:
everyone presses play at the same moment, and everyone stays on the same frame.

It does not host films and it never touches your Jellyfin or Plex. Each person
keeps streaming from their own library. Flick Server only answers one
question: *who is in the room, what are they watching, and where are we?*

## Highlights

### 🎬 Everyone on the same frame
Play, pause, seek and speed are shared with the whole room. Small drifts are
corrected **smoothly** with a barely noticeable speed change. A real seek only
happens when someone is far behind.

### 🔗 One link to invite everyone
No account to create, no key to type. On first start Flick Server makes its
own key and gives you a **single invitation link**. Paste it into Flick and you
are connected. A QR code is one command away.

### 🖥️ A web panel, if you want one
Turn on the optional panel to copy the invitation, see the live rooms, close
a stuck one and watch sync statistics, in the same design as Flick. Off by
default: when disabled, nothing listens.

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
- **No media passes through it.** Only room state and a reference to the title.
- **Nothing is stored.** Rooms live in memory and vanish on restart. The only
  file written is the signing key.
- **Short-lived signed tokens.** Access is checked on every request.
- **Panel locked behind a password**, and off unless you enable it.

## Get started

You need Docker and a domain name pointing to your server.

```sh
cp .env.example .env                 # set FLICKSYNC_PUBLIC_URL=https://your.domain
docker compose up -d --build
docker compose exec flicksync flicksync invite
```

The last command prints your invitation link. Paste it into Flick, and you
are done.

Want the panel? Add three lines to `.env`:

```sh
ENABLE_WEB_PANEL=true
PANEL_PASSWORD=<10+ characters>
FLICKSYNC_ADMIN_TOKEN=<16+ characters>
```

Using Dokploy, Coolify, a reverse proxy or no Docker at all? See
**[TECHNICAL.md](TECHNICAL.md)** and the [deployment guide](docs/deployment.md).

## Honest limits

- **One server, one process.** Rooms are not shared between several
  instances. That is plenty for a household and a large circle of friends.
- **Rooms do not survive a restart.** Restarting Flick Server ends the rooms
  in progress, and friends simply create a new one.
- **Everyone needs access to the title.** Flick Server does not stream
  anything: each person plays it from their own Jellyfin or Plex.

## Documentation

How it works, every setting, security, deployment and development notes are
in **[TECHNICAL.md](TECHNICAL.md)**. Deeper references live in [`docs/`](docs).

## Built with

[Rust](https://www.rust-lang.org) · [Tokio](https://tokio.rs) ·
[Axum](https://github.com/tokio-rs/axum) · [Next.js](https://nextjs.org) ·
[Docker](https://www.docker.com)

## License

Flick Server is free software under the [GNU General Public License v3.0](LICENSE).

Flick is an independent project. It is not affiliated with Jellyfin or Plex.

## Notes from the dev

Built with Claude, fully brainstormed and thought by a human

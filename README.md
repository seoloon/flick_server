# FlickSync

A small, self-hosted **watch-together synchronization service** for the Flick client.
It is not Jellyfin and not Plex and never touches them: it only coordinates *who is in a room, what they are
watching (an opaque media reference) and where the shared playback position is*. Each Flick client resolves and
plays the media through its own Jellyfin/Plex connection.

```
Person A creates a room → friends join → host picks a movie/episode → everyone opens it →
play / pause / seek / speed are shared → small drifts are corrected smoothly
```

Rust · Tokio · Axum · WebSockets · one process · everything in memory · no database, no Redis.

## Quick start

```bash
cp .env.example .env            # set FLICKSYNC_AUTH_KEYS
docker compose up -d --build    # or: cargo run --release
curl localhost:8787/health
```

Try it without a Flick Server:

```bash
# key in .env: FLICKSYNC_AUTH_KEYS=main:my-flick:<secret of 32+ chars>
TOKEN=$(cargo run -q --example mint_token -- main <secret> my-flick alice Alice)
curl -s -X POST localhost:8787/api/v1/rooms -H "Authorization: Bearer $TOKEN"
# then connect a WebSocket client to the returned ws_path with the same Authorization header
```

## Documentation

| Document | For |
|---|---|
| [docs/protocol.md](docs/protocol.md) | Flick client developers: every message, payload, error and the sync model |
| [docs/flick-integration.md](docs/flick-integration.md) | What Flick (client + server) must implement; token issuing and key rotation |
| [docs/deployment.md](docs/deployment.md) | Docker, Dokploy, Coolify, bare metal, Nginx/Traefik/Cloudflare WebSocket setup, operations |
| [docs/architecture.md](docs/architecture.md) | Design, decisions, risks |
| [docs/scaling.md](docs/scaling.md) | What multi-instance rooms would require |
| [docs/openapi.yaml](docs/openapi.yaml) | REST API (OpenAPI 3) |
| [.env.example](.env.example) | Every configuration variable, documented |

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test            # unit + integration (real WebSockets) + protocol + concurrency
```

Requires Rust ≥ 1.88 (edition 2024). The synchronization logic (`sync/`, `room/room.rs`) is pure and uses an
injectable clock, so its tests are deterministic and do not sleep.

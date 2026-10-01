# Deploying FlickSync

FlickSync is a single, small, stateless-at-the-infrastructure-level service: one process, everything in
memory, no database, no Redis. Rooms are ephemeral; a restart ends all rooms (clients see the socket drop,
reconnect, and get `ROOM_NOT_FOUND`).

## Simplest path: Docker Compose on a VPS

```bash
git clone <repo> flicksync && cd flicksync
cp .env.example .env
# edit .env: at least FLICKSYNC_AUTH_KEYS (see docs/flick-integration.md)
docker compose up -d --build
curl http://localhost:8787/health        # {"status":"ok",...}
```

The image is multi-stage (Rust build → `distroless/cc` runtime), runs as a non-root user, has no shell or package
manager, and carries a `HEALTHCHECK` (`flicksync healthcheck`, a built-in probe, since the image has no curl).
`docker-compose.yml` also runs it read-only with all capabilities dropped. Put a TLS-terminating reverse proxy in
front (below): Flick clients should use `https://` / `wss://`.

Update: `git pull && docker compose up -d --build`.

Configuration is entirely through `FLICKSYNC_*` environment variables, documented in `.env.example`.
Required: `FLICKSYNC_AUTH_KEYS` (the service refuses to start without a signing key).

## Dokploy / Coolify

1. Create an application from the Git repository using the **Dockerfile** build type (or the Compose file).
2. Set the environment variables from `.env.example` (at least `FLICKSYNC_AUTH_KEYS`; keep it a *secret* variable).
3. Container port: `8787` (or your `FLICKSYNC_PORT`). Attach your domain; the platform's Traefik/Caddy handles TLS.
4. Health check path: `/health` (platforms that use the Docker `HEALTHCHECK` need nothing).
5. Run **one** replica (rooms live in the memory of one instance, see [scaling.md](scaling.md)).

WebSockets work through Traefik (Dokploy, Coolify) without extra configuration.

## Bare-metal Linux

```bash
cargo build --release          # Rust ≥ 1.88
sudo install -m 0755 target/release/flicksync /usr/local/bin/
```

`/etc/systemd/system/flicksync.service`:

```ini
[Unit]
Description=FlickSync
After=network-online.target
Wants=network-online.target

[Service]
User=flicksync
DynamicUser=yes
EnvironmentFile=/etc/flicksync.env      # chmod 600, contains FLICKSYNC_AUTH_KEYS
ExecStart=/usr/local/bin/flicksync
Restart=on-failure
RestartSec=2
TimeoutStopSec=15
# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
LimitNOFILE=65536
```

`systemctl enable --now flicksync`. SIGTERM/SIGINT trigger a graceful shutdown (rooms closed, sockets drained).
Raise `LimitNOFILE` if you expect thousands of simultaneous connections.

## Reverse proxy and WebSockets

The WebSocket endpoint is `GET /api/v1/rooms/{id}/ws` (an HTTP/1.1 `Upgrade`). The proxy must forward the
upgrade and must not time out idle connections faster than FlickSync pings (every 20 s by default).

### Nginx

```nginx
map $http_upgrade $connection_upgrade { default upgrade; '' close; }

server {
    listen 443 ssl http2;
    server_name sync.example.com;
    # ssl_certificate ... ;

    location / {
        proxy_pass http://127.0.0.1:8787;
        proxy_http_version 1.1;                       # required for WebSockets
        proxy_set_header Upgrade $http_upgrade;       # required
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 3600s;                     # default 60s would cut idle sockets
        proxy_send_timeout 3600s;
        proxy_buffering off;
    }
}
```

### Traefik (Docker labels)

```yaml
labels:
  - traefik.enable=true
  - traefik.http.routers.flicksync.rule=Host(`sync.example.com`)
  - traefik.http.routers.flicksync.entrypoints=websecure
  - traefik.http.routers.flicksync.tls.certresolver=letsencrypt
  - traefik.http.services.flicksync.loadbalancer.server.port=8787
```
Traefik proxies WebSockets natively. Do not add a response-compression or buffering middleware to this router.

### Cloudflare proxy

WebSockets are supported on all plans (enable "WebSockets" in Network if it is off). Cloudflare drops connections that
are idle for ~100 s; FlickSync's 20 s ping keeps them open. Keep `FLICKSYNC_WS_PING_INTERVAL` well below 100.

### Caddy

```
sync.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

## Operations

- **Health**: `GET /health` (liveness), `GET /ready` (503 when shutting down or without keys).
- **Logs**: structured via `tracing`; `FLICKSYNC_LOG_FORMAT=json` for log shippers, `FLICKSYNC_LOG_LEVEL=info`
  (or `info,flicksync::room=debug`). Events include `room_id`, `participant_id` and `server_id`. Tokens, passwords and chat
  contents are never logged (and query strings are not logged).
- **Metrics** (optional): `FLICKSYNC_METRICS_ENABLED=true` exposes Prometheus text at `GET /metrics`
  (`rooms_active`, `participants_active`, `websocket_connections`, `rooms_created_total`, `messages_received_total`,
  `sync_corrections_total`, `rtt_average_milliseconds`, `auth_failures_total`, …). Protect it with
  `FLICKSYNC_METRICS_TOKEN` or expose it only on the internal network.
- **Client IPs**: FlickSync rate-limits per authenticated user, not per IP. Throttle anonymous traffic (failed logins,
  connection floods) in the reverse proxy / firewall, e.g. `limit_req` in Nginx.
- **CORS / Origin**: `FLICKSYNC_CORS_ORIGINS=https://app.example.com` for browser-hosted clients. Native clients need nothing.
  The same list gates the WebSocket `Origin` header; `*` is rejected at startup.
- **Capacity**: each connection costs a few KiB; a small VPS handles thousands of connections. Tune
  `FLICKSYNC_MAX_CONNECTIONS`, `FLICKSYNC_MAX_ROOMS`, `FLICKSYNC_MAX_ROOM_SIZE`.

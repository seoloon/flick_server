# Deploying FlickSync

FlickSync is a single, small, stateless-at-the-infrastructure-level service: one process, everything in
memory, no database, no Redis. Rooms are ephemeral; a restart ends all rooms (clients see the socket drop,
reconnect, and get `ROOM_NOT_FOUND`).

## Simplest path: Docker Compose on a VPS

```bash
git clone <repo> flicksync && cd flicksync
cp .env.example .env
# edit .env: set FLICKSYNC_PUBLIC_URL=https://sync.example.com (your public address)
docker compose up -d --build
curl http://localhost:8787/health        # {"status":"ok",...}
docker compose exec flicksync flicksync invite   # prints the invitation link for Flick
```

The invitation link is also printed in a banner in the startup logs (`docker compose logs flicksync`).

The image is multi-stage (Rust build → `distroless/cc` runtime), runs as a non-root user, has no shell or package
manager, and carries a `HEALTHCHECK` (`flicksync healthcheck`, a built-in probe, since the image has no curl).
`docker-compose.yml` also runs it read-only with all capabilities dropped. Put a TLS-terminating reverse proxy in
front (below): Flick clients should use `https://` / `wss://`.

Update: `git pull && docker compose up -d --build`.

Configuration is entirely through `FLICKSYNC_*` environment variables, documented in `.env.example`.
Nothing is required. Without `FLICKSYNC_AUTH_KEYS` / `FLICKSYNC_AUTH_KEYS_FILE`, a signing key is generated on
first start into `FLICKSYNC_DATA_DIR` (default `./data`, `/data` in the image; file `auth_keys`, mode 0600) and
reused afterwards. **Keep that directory on a persistent volume** (the compose file does): if it is lost, a new key
is generated and the old invitation stops working. The service refuses to start if the file exists but is empty or
corrupt; it never regenerates a key silently. Set `FLICKSYNC_PUBLIC_URL` so invitations carry your real address.

### Web panel

`ENABLE_WEB_PANEL=true` in `.env`, plus `PANEL_PASSWORD` and `FLICKSYNC_ADMIN_TOKEN`, starts the `panel` service on
port 3000 (see [../panel/README.md](../panel/README.md)); without it the service exits immediately and nothing is
published. Put the panel behind your TLS reverse proxy like FlickSync, and forward `X-Forwarded-Proto` and
`X-Forwarded-For`. Use `docker compose up -d flicksync` to run FlickSync alone and skip building the panel.

### Invitation and key management

| Need | How |
|---|---|
| Show the invitation | startup banner in the logs, or `flicksync invite` (`docker compose exec flicksync flicksync invite`) |
| QR code in the terminal | `flicksync invite --qr` |
| Rotate | `flicksync invite --rotate` adds a key (new kid, same server id), keeps the old one valid and prints the new invitation; **restart** the service so it loads the new key |

The secret is printed only by the banner and by `flicksync invite`; logs and errors show `<redacted>`.
There is deliberately no HTTP endpoint returning the key. With `FLICKSYNC_AUTH_KEYS[_FILE]` set, nothing is
generated, the banner is not printed (existing installs behave exactly as before), `flicksync invite` builds the
link from the newest configured key, and `--rotate` is refused (rotate in your own configuration).

`/ready` stays 503 until a key is loaded.

## Dokploy / Coolify

1. Create an application from the Git repository using the **Dockerfile** build type (or the Compose file).
2. Set `FLICKSYNC_PUBLIC_URL` (your domain) and mount a persistent volume on `/data` (holds the generated signing key). Read the invitation from the logs or run `flicksync invite` in the container terminal. Alternatively set `FLICKSYNC_AUTH_KEYS` yourself as a *secret* variable.
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
EnvironmentFile=/etc/flicksync.env      # chmod 600; set FLICKSYNC_DATA_DIR=/var/lib/flicksync and FLICKSYNC_PUBLIC_URL
StateDirectory=flicksync               # persistent /var/lib/flicksync for the generated key
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

## One domain for several components (Traefik)

FlickSync is not the only component of a Flick Server, so a single domain can serve them side by side, each under its
own path prefix, without a second subdomain. The panel keeps the root; FlickSync lives under `/sync` and the proxy strips
the prefix, so FlickSync itself needs no change and no extra hop:

```yaml
# docker-compose.override.yml (Traefik v3, entrypoint websecure, certresolver le)
services:
  flicksync:
    ports: !reset []        # Traefik reaches it over the Docker network (Compose 2.24+)
    labels:
      - traefik.enable=true
      - traefik.http.routers.flicksync.rule=Host(`flick.example.com`) && PathPrefix(`/sync`)
      - traefik.http.routers.flicksync.entrypoints=websecure
      - traefik.http.routers.flicksync.tls.certresolver=le
      - traefik.http.routers.flicksync.priority=100
      - traefik.http.routers.flicksync.middlewares=flicksync-strip
      - traefik.http.middlewares.flicksync-strip.stripprefix.prefixes=/sync
      - traefik.http.services.flicksync.loadbalancer.server.port=8787
  panel:
    ports: !reset []
    labels:
      - traefik.enable=true
      - traefik.http.routers.panel.rule=Host(`flick.example.com`)
      - traefik.http.routers.panel.entrypoints=websecure
      - traefik.http.routers.panel.tls.certresolver=le
      - traefik.http.services.panel.loadbalancer.server.port=3000
```

Then set `FLICKSYNC_PUBLIC_URL=https://flick.example.com/sync` so the invitation link carries the prefix
(`flicksync://flick.example.com/sync/?v=1&tls=1#k=...`). Clients append every API path, `ws_path` included, to that
base. WebSockets pass through Traefik without extra configuration. The `ports: !reset []` lines unpublish the host ports: only Traefik
reaches the containers. The compose services must also be on a network Traefik shares (add `networks:` as in your
Traefik setup).

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

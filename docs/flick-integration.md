# Integrating FlickSync into Flick

What the Flick Server and the Flick client must implement. The wire details are in
[protocol.md](protocol.md); this document is the *to-do list* and the division of responsibilities.

## Responsibilities

| FlickSync (this service) | Flick (client + Flick Server) |
|---|---|
| Rooms, participants, presence, host/permissions | Login, user identity, Flick Server configuration |
| Verifying tokens **locally** | **Issuing** the FlickSync token (the Flick Server owns the signing secret) |
| Canonical playback state (position/state/rate + sequence) | Rendering the video, local buffering, the actual seeking |
| Ordering of concurrent commands | Sending user intent (play/pause/seek/rate) |
| Drift detection and correction *requests* | Applying corrections smoothly |
| Chat transport, history (in memory), flood control | Chat UI, markup-free rendering of `text` |
| Carrying an opaque `MediaRef` | **Resolving** the `MediaRef` through its own Jellyfin/Plex connection and playing it |
| — | Showing participants, host crown, connection state |

FlickSync never contacts Jellyfin/Plex, never fetches URLs and never proxies media. It does not know
the user's media server address or credentials.

## 1. Configure the FlickSync server

In Flick's settings the user (or the Flick Server admin) gives the FlickSync base URL, e.g.
`https://sync.example.com`. The WebSocket URL is derived from it (`https`→`wss`, `http`→`ws`) plus the
`ws_path` returned by the API. Health probe: `GET /health`.

## 2. Authentication — what the Flick Server must do

The Flick Server **mints a short-lived JWT** for the logged-in user and hands it to the client. The
client never holds the secret.

```
Header : { "alg": "HS256", "kid": "main" }
Claims : { "sub": "<flick user id>", "server_id": "<this Flick Server's id>", "aud": "flicksync",
           "name": "<display name>", "perms": ["rooms:create","rooms:join","chat:send"],
           "iat": 1790887000, "exp": 1790890600 }
```

FlickSync is configured with the matching key: `FLICKSYNC_AUTH_KEYS=main:<server_id>:<secret>`.
Rules enforced by FlickSync: HS256 only; `kid` must be known; `server_id` must equal the one bound to
that key (so a client or another Flick Server cannot impersonate this one); `aud` must match; the token
must not be expired, nor valid for longer than `FLICKSYNC_AUTH_MAX_TOKEN_TTL` (default 24 h). Use 1 hour or less.

Example (Node, `jose`):

```ts
import { SignJWT } from "jose";

export async function flickSyncToken(user: { id: string; name: string }) {
  return new SignJWT({
      server_id: process.env.FLICK_SERVER_ID,
      name: user.name,
      perms: ["rooms:create", "rooms:join", "chat:send"],
    })
    .setProtectedHeader({ alg: "HS256", kid: process.env.FLICKSYNC_KID })
    .setSubject(user.id)
    .setAudience("flicksync")
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(new TextEncoder().encode(process.env.FLICKSYNC_SECRET));
}
```

For a quick manual test without a Flick Server:
`cargo run --example mint_token -- main <secret> <server_id> alice Alice`.

Give *view-only* users `perms: ["rooms:join"]`; leave out `chat:send` to mute someone.
Nothing is granted by default.

### Key rotation

1. Generate a new secret (`openssl rand -base64 48`).
2. Add it to FlickSync **next to** the old one with a new kid and the **same** server id:
   `FLICKSYNC_AUTH_KEYS=main:my-server:OLD,main2:my-server:NEW`, restart FlickSync (rooms are in memory:
   restart in a quiet moment; clients reconnect, see "Reconnection").
3. Switch the Flick Server to sign with `kid=main2`.
4. After the longest token lifetime has passed, remove the old key and restart.

Keys can be supplied from a file (`FLICKSYNC_AUTH_KEYS_FILE`, one `kid:server_id:secret` per line) for
Docker/Kubernetes secrets. One FlickSync instance can serve several Flick Servers (one or more keys each);
each Flick Server's rooms are isolated from the others.

## 3. Create / join a room

```http
POST /api/v1/rooms
Authorization: Bearer <token>
Content-Type: application/json

{ "control_mode": "everyone", "chat_enabled": true }      ← body and both fields are optional
```
→ `201`
```json
{ "room_id": "MR82A9Z8VBJQ", "share_code": "MR82-A9Z8-VBJQ", "participant_id": "alice",
  "host_id": "alice", "ws_path": "/api/v1/rooms/MR82A9Z8VBJQ/ws", "room": { …RoomView… } }
```

Show `share_code` (or a deep link containing it) for the user to share. A friend joins with
`POST /api/v1/rooms/{room_id or share_code}/join` (same response shape, status `200`). The code is case-
insensitive and hyphens are ignored. Join errors to surface in the UI: `404` unknown/expired room, `409
ROOM_FULL`, `403 FORBIDDEN` (token without `rooms:join`), `401` (refresh the token).

`POST …/leave` leaves explicitly; `GET /api/v1/rooms/{id}` returns the room state to members.

## 4. Open the WebSocket and receive the room state

Connect to `ws_path` with `Authorization: Bearer <token>` (native clients) or `?access_token=` (browser-like
clients that cannot set headers). The first messages are `room_state` and `chat_history`.

- Show `room.participants` with `presence` and the host (`is_host`).
- If `room.media` is set, go to step 5 immediately (late joiner).
- Keep following `participant_joined`, `participant_left`, `presence_changed`, `room_updated`.

## 5. Resolve the media and start playback

The host picks a movie/episode from Flick's own Jellyfin/Plex library and sends:

```json
{ "type": "select_media", "payload": { "media": {
    "provider": "plex", "server_id": "<plex machine id>", "media_id": "<ratingKey>",
    "media_type": "episode", "season_id": "<optional>", "episode_id": "<optional>", "title": "S02E05" } } }
```

Every client receives `media_selected` and **resolves it itself**: choose the configured connection whose
provider matches `provider` and whose server id matches `server_id`. If the user has no access to that server
or item, show a clear message ("this title is not available on your server") and stay in the room (you can
still chat); do not try other providers — ids are not interchangeable.

Then wait for the item to be ready (opened, buffered enough to start) and apply the playback snapshot:

```
target = position + (server_now − server_time)/1000 × rate     // when state == "playing"
```

where `server_now = client_now + offset` from the ping/pong clock estimate
([protocol.md §5](protocol.md#what-a-client-does)).

## 6. Local controls → commands

Map the player controls to messages and **do not apply them locally first**: send the command and apply the
resulting broadcast (`playback_*`, which also comes back to the sender). This is what makes everyone, including the
initiator, land on the same canonical state, and it resolves conflicts. A guest-initiated pause when the room is
`host_only` returns `CONTROL_DENIED`: show "only the host can control playback" and resync with `sync_request`.

| User action | Message |
|---|---|
| Play | `playback_play` (`{}` or `{position}`) |
| Pause | `playback_pause` (`{position}` = where the user stopped) |
| Seek / scrub release | `playback_seek { position }` — send once on release, not while dragging |
| Speed menu | `playback_rate_changed { rate }` |

Programmatic changes you make yourself (applying a correction, seeking to the target) must **not** be echoed
back as commands: keep a flag that distinguishes them from user input.

## 7. Report local playback and apply corrections

While playing, every ~5 s send `sync_report { position, sequence, state, rtt_ms }`; also send one after you finish
a seek or buffering. Do not send reports while buffering (`buffering: true`).

Apply `sync_correction`:

- `adjust_rate`: set the player speed to `rate` for `duration_ms`, then back to the room rate. Changes of ≤ 8 % are
  inaudible/invisible with pitch correction on; keep it on.
- `seek`: seek to `position`.

Ignore corrections older than the last snapshot you applied (`sequence`). Do not seek on your own for small drifts:
that is the server's job, and avoiding hard seeks is what makes the experience feel smooth.

If a `sync_state` arrives (heartbeat/stale), treat it like any snapshot (re-derive the target; usually nothing to do).

## 8. Disconnect / reconnect

Treat any socket loss as temporary: keep the player in its current state (do not pause automatically, unless you
want to show a "reconnecting…" overlay), reconnect with back-off and a **fresh token**, and re-apply the new
`room_state` exactly like step 5. The seat is kept for 30 s (`FLICKSYNC_RECONNECT_GRACE`), and the room keeps its
canonical timeline meanwhile: a client returning after 5 s is told the position 5 s later.

Close codes to handle specially: `4001` (another device took over: do not reconnect automatically), `4002`/`4003`
(left / room closed: back to the lobby). `ROOM_NOT_FOUND` on reconnect means the room is gone.

## 9. Participants and chat UI

- Participants: name (`display_name`), host badge, presence dot (`connected` / `reconnecting` → greyed out).
- Chat: render `text` as **plain text**; show `sender_name` and `timestamp` (local time zone); keep the last
  `chat_history` plus new messages. Handle `RATE_LIMITED` (slow down) and `CHAT_DISABLED`/`FORBIDDEN` (hide the input).
- Host-only controls: media picker, "everyone can control / only me" toggle (`update_room`), "close room"
  (`close_room`). Hide them for guests; the server enforces it anyway.

## 10. Checklist

- [ ] Flick Server: shared secret + `kid` + `server_id` configured on both sides; token endpoint for logged-in users.
- [ ] Client: FlickSync URL in settings; create / join (code) / leave screens.
- [ ] Client: WebSocket with auto-reconnect, back-off, fresh token, close-code handling.
- [ ] Client: clock offset/RTT estimation (`ping`), snapshot → target position derivation, stale-sequence guard.
- [ ] Client: media resolution via Jellyfin/Plex from `MediaRef`, "unavailable" fallback.
- [ ] Client: command sending without local echo; correction handling (`adjust_rate`, `seek`); periodic `sync_report`.
- [ ] Client: participants list with presence, chat, host-only controls.
- [ ] Ops: HTTPS/WSS in front of FlickSync ([deployment.md](deployment.md)).

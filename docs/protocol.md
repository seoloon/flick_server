# FlickSync WebSocket protocol — version 1

This document is the contract between FlickSync and the Flick client. Everything here is
implemented and covered by tests (`tests/integration.rs`, `tests/protocol.rs`).

- [1. Connection flow](#1-connection-flow)
- [2. Authentication](#2-authentication)
- [3. Frame format and versioning](#3-frame-format-and-versioning)
- [4. Room lifecycle](#4-room-lifecycle)
- [5. Synchronization model](#5-synchronization-model)
- [6. Sequence numbers](#6-sequence-numbers)
- [7. Server → client messages](#7-server--client-messages)
- [8. Client → server messages](#8-client--server-messages)
- [9. Permissions](#9-permissions)
- [10. Host behavior](#10-host-behavior)
- [11. Reconnection and presence](#11-reconnection-and-presence)
- [12. Limits and abuse protection](#12-limits-and-abuse-protection)
- [13. Error codes](#13-error-codes)
- [14. Close codes](#14-close-codes)

## 1. Connection flow

```
Flick client            Flick Server                FlickSync
     │  login (existing)     │                          │
     │──────────────────────▶│                          │
     │  FlickSync token (JWT)│                          │
     │◀──────────────────────│                          │
     │                                                  │
     │  POST /api/v1/rooms            (create)   or     │
     │  POST /api/v1/rooms/{id}/join  (join)            │
     │─────────────────────────────────────────────────▶│  → room id, participant id, ws_path, room state
     │                                                  │
     │  GET  /api/v1/rooms/{id}/ws   (Upgrade, Authorization: Bearer <token>)
     │─────────────────────────────────────────────────▶│
     │◀──── room_state  (canonical state: media, playback, host, participants)
     │◀──── chat_history
     │──── ping ───────────────────────────────────────▶│  estimate RTT / clock offset
     │◀─── pong ────────────────────────────────────────│
     │   resolve media with Jellyfin/Plex, start playback at the derived target position
     │──── sync_report (every ~5 s while playing) ─────▶│
     │◀─── sync_correction (only when needed) ──────────│
     │◀─── sync_state (heartbeat every 10 s while playing)
```

- The REST calls are optional: opening the WebSocket for a room you are not yet in joins it
  implicitly (subject to the same checks). REST `join` is useful to reserve a seat and to learn
  the room state before connecting.
- A participant that joined over REST but never opens a socket is removed after
  `FLICKSYNC_CONNECT_GRACE` (60 s).
- Rejections that can be detected before the upgrade are plain HTTP errors (`401`, `403`, `404`,
  `409` room full, `410` closed, `503`); the body is `{"error":{"code":"…","message":"…"}}`.

## 2. Authentication

FlickSync has no accounts. It trusts a signed token issued by the Flick Server and verifies it
locally (no call to the Flick Server).

| Where | How |
|---|---|
| REST | `Authorization: Bearer <token>` |
| WebSocket | `Authorization: Bearer <token>` on the upgrade request, **or** `?access_token=<token>` for clients that cannot set headers (browsers). Prefer the header: URLs end up in proxy logs. FlickSync itself never logs query strings or tokens. |

Token = JWT, **HS256 only**, header `kid` required.

| Claim | Required | Meaning |
|---|---|---|
| `sub` | yes | Flick user id (`[A-Za-z0-9._:@-]`, ≤ 128 chars). Becomes the `participant_id`. |
| `server_id` | yes | The issuing Flick Server. Must equal the `server_id` bound to the signing key. |
| `aud` | yes | Must be `flicksync` (configurable). |
| `exp` | yes | Expiry (Unix seconds). Must be in the future and ≤ `FLICKSYNC_AUTH_MAX_TOKEN_TTL` away. |
| `name` | no | Display name (sanitized, ≤ 64 chars); defaults to `sub`. |
| `perms` | no | List of `rooms:create`, `rooms:join`, `chat:send`, or `*`. **No permission is granted by default.** |

The token is verified when the connection is established. Mint short-lived tokens and request a
fresh one before every (re)connection. Details and key rotation: [flick-integration.md](flick-integration.md).

A room belongs to the Flick Server of its creator: users whose token carries a different
`server_id` get `404` for that room (existence is not revealed).

## 3. Frame format and versioning

Text frames only, UTF-8 JSON. Binary frames are answered with an `INVALID_MESSAGE` error.

```json
{ "protocol_version": 1, "type": "playback_pause", "payload": { "position": 123.42 } }
```

- Server → client frames always carry `protocol_version`.
- Client → server: `protocol_version` is optional (defaults to `1`); a different value yields
  `UNSUPPORTED_VERSION`. `payload` may be omitted when a message has no required fields.
- **Evolution rules** (so older clients keep working): fields are only ever *added*; clients must
  ignore unknown fields and unknown message types; the server ignores unknown fields in client
  payloads. A breaking change would bump `protocol_version`.
- Numbers: positions are seconds (float), `server_time`/`timestamp` are milliseconds since the Unix
  epoch, `*_ms` fields are milliseconds, `rate` is a multiplier (1.0 = normal).
- Maximum inbound frame: `FLICKSYNC_WS_MAX_MESSAGE_BYTES` (16 KiB). Larger frames close the connection.

## 4. Room lifecycle

```
CREATE ─▶ waiting ─▶ media_selected ─▶ playing ⇄ paused ─▶ empty ─▶ (destroyed)
                          ▲                │                 │
                          └── new media ───┘      someone joins in time ─▶ back to the previous state
```

| `state` | Meaning |
|---|---|
| `waiting` | Room exists, no media chosen. |
| `media_selected` | Media chosen, nobody pressed play yet. |
| `playing` / `paused` | Canonical playback status. |
| `empty` | No participant left; destroyed after `FLICKSYNC_ROOM_TIMEOUT` (60 s) unless someone joins. |
| `closed` | Terminal (shown in `room_closed`; the room is gone). |

Rooms also expire after `FLICKSYNC_ROOM_IDLE_TIMEOUT` (12 h) without any client message
(`room_closed` with reason `expired`).

## 5. Synchronization model

The server is authoritative. Playback is a *state with a reference timestamp*, not a stream of positions:

```
PlaybackSnapshot = { state, position, rate, server_time, sequence }

canonical_position(t) = position + (t − server_time) / 1000 × rate     (state == "playing")
                      = position                                         (state == "paused")
```

`server_time` is the server wall clock (ms) at which `position` was valid. Internally the server
measures every elapsed interval with a monotonic clock, so system clock adjustments do not
disturb playback; the wall-clock value is only communicated for the clients.

### What a client does

1. **Clock offset.** Send `ping { client_time, rtt_ms? }`. On `pong { client_time, server_time }`:
   ```
   rtt    = now_client − client_time            (same unit/clock as client_time, in ms)
   offset = server_time − (client_time + rtt / 2)
   server_now = now_client + offset
   ```
   Repeat a few times at start and keep the sample with the lowest RTT; then every ~30 s.
   Pass the smoothed RTT in `ping.rtt_ms` and `sync_report.rtt_ms`.
2. **Apply a snapshot** (from `room_state`, any `playback_*` event, `media_selected`, `sync_state`):
   ```
   if snapshot.sequence < lastSequence: ignore (stale)
   lastSequence = snapshot.sequence
   target = canonical_position(server_now)
   set playback rate = snapshot.rate; seek to target (if the local position differs noticeably); play or pause
   ```
3. **Report** every ~5 s while playing (and after a seek/buffering finishes):
   `sync_report { position, sequence, state, buffering?, rtt_ms? }`. Do not report while buffering
   (set `buffering: true` or stay silent).
4. **Obey corrections** (`sync_correction`), see below.

### Drift correction (server side)

On a `sync_report` the server computes `drift = client_position + rtt/2·rate − canonical_position`
(positive = client ahead) and answers with the gentlest effective correction. Thresholds are
configurable (`FLICKSYNC_SYNC_DRIFT_*`); defaults:

| \|drift\| | Reaction |
|---|---|
| < 100 ms | nothing |
| 100–500 ms | `sync_correction { action: "adjust_rate" }`, ±3 % rate |
| 500–1500 ms | `adjust_rate`, ±8 % rate |
| ≥ 1500 ms | `sync_correction { action: "seek" }` to the latency-compensated canonical position |

Anti-jitter rules: a rate correction is not re-issued while the previous one is still running
(`duration_ms`), and a participant is hard-seeked at most once per `FLICKSYNC_SYNC_SEEK_COOLDOWN_MS`
(3 s). While the room is paused only a `seek` can be requested (when the drift is ≥ 500 ms).
If the report carries an old `sequence` or a `state` that differs from the room's, the server sends a
full `sync_state` (`reason: "stale"`) instead of a correction.

`adjust_rate`: set the local rate to `rate` for `duration_ms`, then return to the room rate.
`seek`: jump to `position`. Ignore a correction whose `sequence` is older than the last one you applied.

## 6. Sequence numbers

Every change of the room playback state increments a room-wide `sequence` (media selection, play,
pause, seek, rate change). It starts at `0` when the room is created.

- The server processes each room's commands one at a time, so concurrent commands get a total
  order. The last command in that order wins for everybody ("A seeks to 500, B to 700, C to 600" ⇒
  all clients end at whichever arrived last, and all see the same order).
- Clients use `sequence` to discard stale snapshots (`snapshot.sequence < lastSequence`).
- Clients *may* put the last sequence they applied into `playback_play|pause|seek|rate_changed`
  (`sequence`) and `sync_report`. A value **ahead of the server's** is a bug or an attack and gets
  `INVALID_SEQUENCE`. A value behind is accepted (last writer wins).
- A redundant command (play while playing, pause while paused, same rate) does not change the state, does
  not increment the sequence and is answered with a `sync_state` (`reason: "stale"`) to the sender only.

## 7. Server → client messages

`PlaybackSnapshot` fields (`state`, `position`, `rate`, `server_time`, `sequence`) are written
*flat* in the payloads below where marked `+snapshot`.

### `room_state`
Sent right after (re)connecting. Full canonical state.
```json
{ "room": {
    "room_id": "MR82A9Z8VBJQ", "state": "playing", "host_id": "alice",
    "control_mode": "everyone", "chat_enabled": true, "max_participants": 100,
    "participants": [
      { "participant_id": "alice", "display_name": "Alice", "presence": "connected", "is_host": true, "joined_at": 1790887544683 }
    ],
    "media": { "provider": "jellyfin", "server_id": "…", "media_id": "…", "media_type": "movie", "title": "…" },
    "playback": { "state": "playing", "position": 102.0, "rate": 1.0, "server_time": 1790887546683, "sequence": 3 },
    "created_at": 1790887544683 },
  "you": "bob", "server_time": 1790887546683 }
```
`media` is `null` until the host selects something. `presence` ∈ `connected | reconnecting | disconnected`.

### `participant_joined`
`{ "participant": ParticipantView }` — someone joined (they may not be connected yet; see `presence_changed`).

### `participant_left`
`{ "participant_id": "bob", "reason": "left" | "timeout" }` — permanent departure
(`timeout` = reconnection grace period elapsed).

### `presence_changed`
`{ "participant_id": "bob", "presence": "connected" | "reconnecting" | "disconnected" }`.

### `media_selected`
`{ "media": MediaRef, "playback": PlaybackSnapshot, "by": "alice" }` — new media; playback is reset to
paused at 0 (rate kept). Clients must resolve `media` through their own Jellyfin/Plex connection.

### `playback_play` / `playback_pause` / `playback_seek` / `playback_rate_changed`
`{ "by": "<participant who caused it>", +snapshot }`. Broadcast to **everyone including the sender**,
which is how the sender learns the canonical result.
```json
{ "protocol_version": 1, "type": "playback_pause",
  "payload": { "by": "alice", "state": "paused", "position": 123.42, "rate": 1.0, "server_time": 1790887546683, "sequence": 42 } }
```

### `sync_state`
`{ "reason": "heartbeat" | "requested" | "stale", +snapshot }`. `heartbeat`: every
`FLICKSYNC_SYNC_HEARTBEAT` (10 s), to everyone, only while the room is playing. `requested`: reply to
`sync_request`. `stale`: reply to a stale report or a no-op command.

### `sync_correction`
Sent to **one** participant, only when its drift needs fixing.
```json
{ "action": "adjust_rate", "rate": 1.03, "duration_ms": 10000, "drift_ms": -300.0, "sequence": 7, "server_time": 1790887546683 }
{ "action": "seek", "position": 120.1, "drift_ms": -20000.0, "sequence": 7, "server_time": 1790887546683 }
```
`rate`/`duration_ms` only for `adjust_rate`, `position` only for `seek`. `drift_ms` is signed (positive = ahead).

### `room_updated`
`{ "host_id", "control_mode", "chat_enabled", "state", "reason": "host_changed" | "settings_changed" }`.

### `room_closed`
`{ "reason": "host_closed" | "host_left" | "expired" | "empty" | "shutdown" }`. The server then closes the socket with code 4003 (also on `shutdown`: rooms live in memory, so they do not survive a restart).

### `chat_message`
`{ "id": 17, "room_id": "…", "sender_id": "bob", "sender_name": "Bob", "text": "hello", "timestamp": 1790887546683 }`.
`text` is plain text (control and bidi-override characters are stripped): render it as text, never as HTML/markup.

### `chat_history`
`{ "messages": [ChatMessage…] }` — the last `FLICKSYNC_CHAT_HISTORY` (100) messages, sent on (re)connect when chat is enabled. History lives in memory only.

### `pong`
`{ "client_time": <echo>, "server_time": <ms> }`.

### `error`
`{ "code": "NOT_HOST", "message": "Only the room host can select media." }` — only to the sender of the
offending message; the connection stays open (except for the cases in [§12](#12-limits-and-abuse-protection)).

## 8. Client → server messages

| `type` | Payload | Who | Notes |
|---|---|---|---|
| `ping` | `{ client_time: number, rtt_ms?: number }` | any | Answered with `pong`. |
| `select_media` | `{ media: MediaRef }` | **host** | Resets playback to paused at 0. |
| `playback_play` | `{ position?: number, sequence?: number }` | per control mode | Without `position`: resume from the canonical position. |
| `playback_pause` | `{ position?: number, sequence?: number }` | per control mode | `position` = where *this client* stopped; defaults to canonical. |
| `playback_seek` | `{ position: number, sequence?: number }` | per control mode | `position` required. |
| `playback_rate_changed` | `{ rate: number, sequence?: number }` | per control mode | Range `FLICKSYNC_RATE_MIN..MAX` (0.25–4.0). |
| `sync_request` | `{}` | any | Answered with `sync_state` (`requested`). Use after buffering/resume. |
| `sync_report` | `{ position: number, sequence?: number, state?: "playing"\|"paused", buffering?: bool, rtt_ms?: number }` | any | May trigger `sync_correction`. |
| `chat_message` | `{ text: string }` | any with `chat:send` | ≤ 500 chars after sanitizing. |
| `update_room` | `{ control_mode?: "everyone"\|"host_only", chat_enabled?: bool }` | **host** | Broadcasts `room_updated`. |
| `close_room` | `{}` | **host** | Closes the room for everyone (`room_closed: host_closed`). |
| `leave_room` | `{}` | any | Leave permanently; socket closed with 4002. |

`MediaRef` — FlickSync only *coordinates identity*; Flick resolves and plays:
```json
{ "provider": "jellyfin" | "plex",
  "server_id": "<Jellyfin/Plex server id>",
  "media_id": "<item id>",
  "media_type": "movie" | "episode",
  "season_id": "optional (episodes only)",
  "episode_id": "optional (episodes only)",
  "title": "optional, display only",
  "duration_secs": 7200.0 }
```
Ids must match `[A-Za-z0-9._:-]{1,128}`: URLs, paths and anything else are rejected with `INVALID_MEDIA`.
Jellyfin and Plex ids are never assumed to be interchangeable. `duration_secs` is optional; when present
the canonical position is clamped to it.

Positions must be finite and within `0 … FLICKSYNC_MAX_POSITION` (7 days) or the command is rejected
with `INVALID_POSITION`.

## 9. Permissions

| Action | Host | Participant |
|---|---|---|
| Select / change media | ✅ | ❌ `NOT_HOST` |
| Play / pause / seek / rate | ✅ | ✅ in `everyone` mode, ❌ `CONTROL_DENIED` in `host_only` |
| Change room settings, close room | ✅ | ❌ `NOT_HOST` |
| Chat | ✅ | ✅ (needs `chat:send` in the token and chat enabled) |
| Leave | ✅ | ✅ |

`control_mode` is chosen at room creation (`POST /api/v1/rooms {"control_mode":"host_only"}`, default
`FLICKSYNC_DEFAULT_CONTROL_MODE`) and can be changed by the host with `update_room`. Every command is
validated server-side against the participant's membership and role; a client that left (or was never
admitted) gets `NOT_MEMBER`.

## 10. Host behavior

The creator is the host. When the host **permanently** leaves (explicit leave, or the reconnection
grace period expires), `FLICKSYNC_HOST_LEAVE_POLICY` decides:

- `transfer` (default): ownership moves to the longest-standing remaining participant, preferring connected
  ones; everyone gets `room_updated { reason: "host_changed" }`.
- `close`: everyone gets `room_closed { reason: "host_left" }`.

A temporary disconnection of the host does **not** transfer ownership while the grace period lasts.
If nobody remains, the room becomes `empty` and is destroyed after `FLICKSYNC_ROOM_TIMEOUT`.

## 11. Reconnection and presence

A participant is identified by their token's `sub` (scoped to the room's Flick Server), not by their socket.

- Socket drops (network loss, app suspended): presence → `reconnecting`, the seat is kept for
  `FLICKSYNC_RECONNECT_GRACE` (30 s). Others receive `presence_changed`.
- The client reconnects with a fresh token to the same `ws_path`: it gets `room_state` (with the
  *current* canonical position) and `chat_history`, others get `presence_changed: connected`, and the
  client re-runs steps 1–2 of [§5](#what-a-client-does). Nothing else needs to be replayed.
- After the grace period: `participant_left { reason: "timeout" }`.
- Connecting again while an old socket for the same user still exists **replaces** it: the old socket
  receives close code 4001, and presence does not flicker.
- Recommended client back-off: 0.5 s, 1 s, 2 s, 4 s, 8 s (± jitter), then every 10 s; stop after the
  grace period has surely elapsed and offer "rejoin" in the UI.

The server pings every `FLICKSYNC_WS_PING_INTERVAL` (20 s) with WebSocket ping frames (clients answer
automatically) and drops sockets silent for `FLICKSYNC_WS_IDLE_TIMEOUT` (60 s). This also keeps
connections alive through proxies with idle timeouts.

## 12. Limits and abuse protection

| Protection | Default | Result |
|---|---|---|
| Inbound frame size | 16 KiB | connection closed |
| Inbound message rate per participant | 20/s, burst 40 | `RATE_LIMITED`; after 20 consecutive violations the socket is closed (1008) |
| Chat rate per participant | 1/s, burst 5 | `RATE_LIMITED` |
| Chat message length / history | 500 chars / 100 messages | `MESSAGE_TOO_LARGE` / oldest dropped |
| Room creation per user | 6/min | `429 RATE_LIMITED` |
| Participants per room | 100 | `409 ROOM_FULL` |
| Rooms per instance | 10 000 | `429 TOO_MANY_ROOMS` |
| WebSocket connections per instance | 10 000 | `503 TOO_MANY_CONNECTIONS` |
| Outbound queue per connection | 256 messages | a connection that cannot keep up is dropped (1013) and treated as disconnected |

Malformed input never terminates a connection by itself: it produces an `error` message.

## 13. Error codes

| Code | HTTP | Meaning |
|---|---|---|
| `UNAUTHENTICATED` | 401 | Missing/invalid/expired token, unknown key, wrong audience or server binding. |
| `FORBIDDEN` | 403 | Token lacks a needed permission, or disallowed `Origin`. |
| `NOT_HOST` | 403 | Host-only action. |
| `NOT_MEMBER` | 403 | Not a participant of the room. |
| `CONTROL_DENIED` | 403 | Room is `host_only` for playback. |
| `ROOM_NOT_FOUND` | 404 | Unknown/malformed room id, or a room of another Flick Server. |
| `ROOM_FULL` | 409 | Room at capacity. |
| `SESSION_REPLACED` | 409 | This socket was superseded by a newer one. |
| `ROOM_CLOSED` | 410 | The room has been closed. |
| `INVALID_MESSAGE` | 400 | Not JSON / not a valid envelope / binary frame. |
| `UNKNOWN_TYPE` | 400 | Unknown message `type`. |
| `UNSUPPORTED_VERSION` | 400 | `protocol_version` ≠ 1. |
| `INVALID_PAYLOAD` | 400 | Wrong shape or field types. |
| `INVALID_POSITION` | 400 | Negative, non-finite or too large position. |
| `INVALID_RATE` | 400 | Rate outside the configured range. |
| `INVALID_SEQUENCE` | 400 | Sequence ahead of the room's. |
| `INVALID_MEDIA` | 400 | Media reference failed validation. |
| `NO_MEDIA` | 400 | Playback command before any media was selected. |
| `MESSAGE_TOO_LARGE` | 413 | Chat message too long. |
| `RATE_LIMITED` | 429 | Rate limit hit. |
| `TOO_MANY_ROOMS` | 429 | Instance room cap reached. |
| `CHAT_DISABLED` | 403 | Chat is off in this room. |
| `TOO_MANY_CONNECTIONS` | 503 | Instance connection cap reached / shutting down. |
| `INTERNAL` | 500 | Unexpected server condition (details are only in the server logs). |

## 14. Close codes

| Code | Meaning | Client should |
|---|---|---|
| 1000 | Normal close (client initiated) | — |
| 1001 | Going away (sent by a reverse proxy or the OS during a restart) | reconnect with back-off |
| 1008 | Policy: attach refused or sustained flooding | do not retry blindly |
| 1013 | Connection too slow, dropped | reconnect |
| 4001 | Replaced by a newer connection of the same user | do **not** auto-reconnect (avoids ping-pong between devices) |
| 4002 | You left / were removed | go back to the lobby |
| 4003 | Room closed | go back to the lobby |

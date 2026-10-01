# FlickSync architecture

## 1. Existing architecture

The repository was empty when FlickSync was started (only the brief). There was no Flick code, auth protocol,
media-id model, API convention, Docker setup or lint configuration to reuse, so this repository defines the
**integration contract** instead: [protocol.md](protocol.md) and [flick-integration.md](flick-integration.md).
If Flick already has equivalents (token format, media identifiers), adapt `auth::Claims` and `protocol::MediaRef` —
they are the only two places that encode those decisions. Rust conventions: stock `rustfmt` and `clippy -D warnings`.

## 2. Layout

```
src/
  main.rs            process entry: config, tracing, `healthcheck` subcommand
  app.rs             AppState wiring, sweeper task, graceful shutdown
  config/            environment → typed config (validated; no operational constants in code)
  auth/              JWT (HS256) verification, key ring bound to Flick Server ids, permissions
  api/               Axum HTTP: health/ready/metrics, rooms REST, bearer extractor, error mapping, CORS
  websocket/         Axum upgrade handler + per-connection task (transport concerns only)
  protocol/          wire types, parsing and validation (versioned envelope, MediaRef)   ← pure
  room/
    room.rs          Room state machine                                                 ← pure
    manager.rs       RoomManager: map of rooms, per-room lock, delivery, sweeping
    tests.rs         deterministic unit tests of the state machine
  sync/
    clock.rs         Clock trait (system / manual), reference client-side offset math  ← pure
    playback.rs      canonical PlaybackState (position + rate + reference + sequence)  ← pure
    drift.rs         drift policy and per-participant hysteresis                       ← pure
  chat/              sanitizing + bounded history                                      ← pure
  ratelimit.rs       token bucket with explicit time                                   ← pure
  metrics.rs         atomic counters/gauges + Prometheus text
  errors/            typed errors with stable codes (shared by REST and WebSocket)
tests/               integration (real sockets), protocol hardening, concurrency
```

Deviations from the layout suggested in the brief, and why: `protocol/` is a top-level module rather than
`websocket/protocol.rs` because the room state machine consumes and produces protocol types and must not depend on
the WebSocket adapter; `room` has no `participant.rs` because a participant is a small private struct of the room;
`storage/` does not exist because there is nothing to store (see §6); `ratelimit.rs` and `metrics.rs` are tiny shared utilities.

### Dependency direction

```
websocket, api ──▶ app/AppState ──▶ room::manager ──▶ room::room ──▶ sync, protocol, chat, ratelimit
        └── auth, config, metrics                         (no sockets, no tasks, no wall clock, no Axum)
```

`Room` receives the current time as an argument (`Time { mono_ms, wall_ms }`) and returns an `Outcome` (messages to
deliver, participants removed, "room is over"). That makes every rule — permissions, sequencing, grace periods, expiry,
heartbeat, drift — testable without async code or sleeping. `RoomManager` is the only place with locks and channels.

## 3. Protocol overview

JSON text frames `{protocol_version, type, payload}`; REST for membership (create/join/leave/get), WebSocket for
everything real-time (media selection, playback, sync, presence, chat). Full reference: [protocol.md](protocol.md).

## 4. Authentication model

The Flick Server signs short-lived HS256 JWTs (`sub`, `server_id`, `aud`, `exp`, `name`, `perms`, header `kid`).
FlickSync verifies them offline with a key ring `kid → (server_id, secret)`. The key is *bound* to a server id and the
`server_id` claim must match it, so a client cannot claim to be another Flick Server; rooms are scoped to the creator's
server id. HS256 only (no algorithm negotiation), minimum 32-character secrets, maximum token lifetime, audience check,
rotation by listing several `kid`s. No user database, no passwords. Tokens are never logged.
*Trade-off*: HS256 means FlickSync holds a secret that could also mint tokens. Asymmetric keys (EdDSA/RS256 with a public key
on FlickSync) would remove that; the `Authenticator` is the only code that would change.

## 5. Synchronization model

Authoritative state `(status, position@reference, rate, sequence)`; clients derive positions locally, nothing streams
positions. Event-driven commands, a 10 s heartbeat while playing, client `sync_report`s every ~5 s answered only when a
correction is needed (rate nudge, stronger nudge, or hard seek with cooldown). Elapsed time is measured with a monotonic clock;
RTT/2 compensation is applied to reports and to seek targets. Details and numbers: [protocol.md §5](protocol.md#5-synchronization-model).

**Ordering.** Each room has one `Mutex`; every command is validated and applied under it, then delivered through
non-blocking `try_send` in the same critical section. This yields a total order per room (and each client's queue preserves it),
and the strictly increasing `sequence` lets clients discard stale snapshots. `tests/concurrency.rs` hammers this with
parallel seeks/play/pause/joins/leaves and checks that all clients observe the same gap-free order.

## 6. Persistence

None. Rooms, chat history and presence are in memory by design (ephemeral, nothing personal stored, trivial to run).
SQLx/SQLite were not added because no requirement needs durable data; if audit logs or persisted rooms are ever wanted,
the seam is `RoomManager` (hooks on create/destroy) and a new `storage/` module.

## 7. Room lifecycle and host policy

`waiting → media_selected → playing ⇄ paused → empty → destroyed`. The host is the creator; when the host leaves for good
ownership transfers (default) or the room closes (`FLICKSYNC_HOST_LEAVE_POLICY`). A dropped socket keeps its seat for a grace
period. Empty rooms are destroyed after a timeout, idle rooms expire. See [protocol.md §4, §10, §11](protocol.md#4-room-lifecycle).

## 8. Decisions made where the brief left room

| Topic | Decision |
|---|---|
| Participant identity | `participant_id` = token `sub`, unique inside a room; one live socket per participant (a new one replaces the old). Survives reconnects. |
| Room id | 12 chars of Crockford base32 from the OS-seeded CSPRNG (60 bits); case-insensitive; `XXXX-XXXX-XXXX` share form. |
| Join semantics | REST `join` reserves a seat; opening the socket also joins implicitly (single code path, no race between the two). |
| Playback permissions | Per-room `control_mode` (`everyone` default / `host_only`), switchable by the host; media selection is always host-only. |
| Client commands vs. broadcast | Commands are not applied locally; the server broadcasts the canonical result to everyone *including the sender*. |
| Redundant commands | No state change, no sequence bump; the sender gets a `sync_state`. |
| Sequence in client messages | Optional; ahead of the server ⇒ `INVALID_SEQUENCE`; behind ⇒ accepted, last writer wins (a strict compare-and-set would reject legitimate concurrent actions). |
| Auth for browsers | `Authorization` header preferred; `?access_token=` fallback; the HTTP layer never logs query strings. |
| CORS / Origin | No wildcard; allow-list from env; also enforced on the WebSocket `Origin` header (browsers ignore CORS there). |
| Metrics | Plain atomics + Prometheus text, off by default, optional bearer token. No metrics crate. |
| Slow clients | Bounded outbound queue per connection; overflow drops the connection (seat kept) rather than blocking the room. |

## 9. Risks and known limitations

- **Token checked at connect time only.** An open socket survives its token's expiry (a 3-hour movie would otherwise be cut
  off). Compromise = short tokens + reconnect with a fresh one; the maximum TTL is capped by config. Revocation before expiry needs a restart or removal of the key.
- **Single instance.** A restart ends all rooms. Documented path to scale out: [scaling.md](scaling.md).
- **Sweeper is O(rooms) per second** (grace/expiry/heartbeat). Fine to tens of thousands of rooms; a timer wheel/heap would be the next step.
- **Per-user, not per-IP, limits.** Anonymous floods (bad tokens) must be absorbed by the reverse proxy; authentication failures are cheap to reject and are counted/logged.
- **Clock-offset error on asymmetric links** is bounded by RTT/2; the drift thresholds (100 ms and up) sit above typical error. They are configurable if real-world data suggests otherwise.
- **Buffering is client-reported.** FlickSync does not pause the room when someone buffers (that is a product decision: wait-for-everyone could be added as a `buffering` state without changing the model).
- **Windows build** is supported for development; the container image targets Linux (glibc).

## 10. Implementation phases (as executed)

1. Repository inspection, architecture, protocol and documentation  
2. Axum server, health, configuration, logging  
3. Authentication (JWT, key ring, permissions)  
4. Room manager: create/join/leave, participant lifecycle  
5. WebSocket protocol  
6. Playback state machine  
7. Synchronization engine (drift policy, heartbeat)  
8. Reconnection and presence  
9. Chat  
10. Rate limiting and security hardening  
11. Tests: unit (≈115), integration (24), protocol (11), concurrency (6)  
12. Docker, compose, deployment docs

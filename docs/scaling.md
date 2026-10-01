# Scaling FlickSync beyond one instance

**V1 is deliberately single-instance and in-memory.** A single small process serves thousands of
connections; this section only explains what multi-instance rooms would require and which seams in the code
are prepared for it, so that none of it needs a rewrite of the synchronization engine.

```
   Load balancer (route by room id)
        │          │          │
   instance A   instance B   instance C
        └──────────┼──────────┘
                 Redis (registry + pub/sub)
```

## What is already isolated

- `room::Room` is a pure state machine: `(state, command, time) → (new state, Outcome)`. It has no sockets,
  tasks or clock. It can run on any node.
- `Outcome.deliveries` is an abstract list of `(Target, ServerMessage)`. Only `RoomManager::dispatch` knows
  how to deliver them (local `mpsc` channels today).
- Every room is mutated by exactly one lock holder, which gives the total command order and the sequence numbers.
- Time is injected (`Clock`); positions are derived from `(position, rate, reference)`, not streamed, so nothing
  depends on node-local state beyond the room itself.
- Room ids are opaque, random and appear in the URL path of every room-scoped request (REST and WebSocket).

## What would be needed

### Room ownership
Exactly one instance must own each room at a time (a *single writer* per room is what preserves ordering and
sequence numbers). Options:

1. **Deterministic placement**: the instance that handles `POST /api/v1/rooms` generates ids whose first characters
   encode the instance (or a shard number), e.g. `shard(2) + random(10)`. Ownership is then derivable from the id.
2. **Registry with leases**: `SET room:{id} {instance} NX PX 30000` in Redis, refreshed every few seconds by the owner.
   If the owner dies, the lease expires and the room is lost (rooms are ephemeral by design) or restored from a periodic snapshot.

### WebSocket routing
Every request for a room must reach its owner. Cheapest: make the load balancer route by room id
(`/api/v1/rooms/{id}/…` — consistent hashing on the path, available in Nginx `hash $room_id consistent`, Envoy, HAProxy,
Traefik with custom headers) so all participants of a room land on the same instance without any inter-node traffic. If the
LB cannot do that, the instance receiving the socket proxies to the owner (or subscribes to the room channel, below).

### Event broadcasting
If participants of one room can be connected to different instances, the owner publishes each `Outcome` delivery on a
per-room pub/sub channel (`room:{id}`), and every instance with local sockets of that room forwards the messages to them.
`dispatch` is the only place to change: `Target::All/AllExcept/Only` map directly onto "publish to channel" with a
recipient filter. Commands from non-owner instances are forwarded to the owner (a stream/queue per room) to keep the single writer.

### Distributed room state
Rooms stay in the owner's memory. For failover, the owner can write a compact snapshot (media, position/rate/status
reference, sequence, participants) to Redis every few seconds; a new owner restores it and clients simply reconnect and
re-sync (the protocol already treats reconnection as normal). Chat history may be included or dropped.
Because positions are derived from a reference instant, restoring a snapshot taken a few seconds ago costs only a
jump of the canonical position, which the correction mechanism smooths out.

### Participant presence
Presence is a property of sockets, so each instance reports its local sockets to the owner (heartbeat over the room
channel, or Redis keys with TTL: `presence:{room}:{user}` refreshed by the connection task). The owner applies the
reconnection grace period exactly as today; only the source of `connect`/`disconnect` events changes.

### Cross-cutting
- **Rate limits** are per participant and per room, i.e. owner-local; room-creation limits per user would need a shared
  counter (Redis `INCR` with expiry) or sticky routing of `POST /rooms` by user id.
- **Capacity gauges** (`max_rooms`, `max_connections`) become per-instance; add global quotas in Redis only if needed.
- **Graceful drain**: on shutdown an instance should hand over or close its rooms (`RoomManager::shutdown` already closes them).
- **Auth** needs nothing: tokens are verified locally with the same keys everywhere.

## Why not now
Single-instance has no ordering ambiguity, no partial failures, trivial operations and the lowest latency. The expected
audience (friends watching together) is orders of magnitude below what one process can serve.

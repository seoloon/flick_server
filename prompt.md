# FlickSync — Claude Code Development Brief

You are the lead backend engineer responsible for designing and implementing FlickSync.

Do not merely generate a prototype. Build a clean, production-oriented foundation that can actually be integrated into the Flick ecosystem.

Before writing significant amounts of code, inspect the repository and understand its existing architecture, conventions, tooling, and documentation. Reuse existing conventions where appropriate, but do not force existing patterns into FlickSync if they are technically inappropriate.

If something is ambiguous, make a technically justified decision and document it rather than stopping unnecessarily.

---

# 1. Product

FlickSync is a standalone server-side companion service for Flick.

It is NOT Jellyfin.
It is NOT Plex.
It does not replace either media server.

It is a real-time synchronization service that allows multiple people using the Flick client to watch the same movie or TV episode together.

The service is hosted separately from Jellyfin/Plex.

Flick clients connect to FlickSync when the user has configured a Flick Server / FlickSync server.

The main use case is:

    Person A creates a room
        ↓
    Person B/C/D/... join
        ↓
    Creator selects a movie or episode
        ↓
    Flick clients open the same media item
        ↓
    Playback is synchronized in real time
        ↓
    Play / pause / seek / playback-rate changes are propagated
        ↓
    FlickSync continuously corrects small timing drifts

The room should support 2, 3, 4, or significantly more participants.

The architecture should not artificially limit the number of participants.

---

# 2. Core philosophy

FlickSync should be:

- lightweight
- fast
- low-latency
- reliable
- easy to self-host
- easy to deploy
- independent from Jellyfin/Plex internals
- secure
- stateless at the infrastructure level where possible
- simple to operate
- designed around real-time events rather than persistent media metadata

Do NOT turn FlickSync into a giant backend.

The service should primarily coordinate:

- rooms
- participants
- playback state
- synchronization
- presence
- chat

It should NOT become a media server.

---

# 3. Technology choice

Use:

- Rust
- Tokio
- Axum
- WebSockets
- Serde
- tracing
- tower / tower-http where useful
- SQLx if persistent storage becomes necessary
- SQLite as the default persistent database if a database is actually required
- Docker / Docker Compose for deployment

Prefer stable, mature crates.

Avoid unnecessary dependencies.

Do not introduce PostgreSQL, Redis, Kafka, NATS, Kubernetes, etc. just because they are common backend technologies.

The first version should run comfortably as a single small service.

However, structure the code so that horizontal scaling could be introduced later without rewriting the entire synchronization engine.

---

# 4. Repository inspection

Before implementation:

1. Inspect the repository.
2. Read all relevant existing documentation.
3. Determine whether Flick already defines:
   - authentication
   - server configuration
   - user identity
   - server URLs
   - API conventions
   - media identifiers
   - playback state structures
   - networking conventions
4. Identify anything that FlickSync should integrate with rather than duplicate.
5. Check existing Rust conventions and formatting/linting configuration.
6. Check whether the repository already contains Docker/deployment infrastructure.

Do not invent APIs that already exist elsewhere in Flick.

If Flick's existing architecture is not present in this repository, design a clean integration contract and document it clearly.

---

# 5. Architecture

Use a layered architecture.

A possible structure:

src/
    main.rs

    config/
    api/
        mod.rs
        health.rs
        rooms.rs
        auth.rs

    websocket/
        mod.rs
        connection.rs
        protocol.rs

    room/
        mod.rs
        manager.rs
        room.rs
        participant.rs

    sync/
        mod.rs
        clock.rs
        playback.rs
        drift.rs

    auth/
        mod.rs

    chat/
        mod.rs

    storage/
        mod.rs

    errors/
        mod.rs

Do not follow this structure blindly if another structure is cleaner.

The important thing is separation of responsibilities.

The synchronization engine must not be tightly coupled to Axum/WebSocket implementation details.

---

# 6. Rooms

A room represents one watch session.

A room contains:

- unique room ID
- creator/host
- participants
- current media
- playback state
- current position
- playback rate
- synchronization timestamp
- room state
- optional chat history
- creation time
- last activity time

Room lifecycle:

    CREATE
       ↓
    WAITING
       ↓
    MEDIA_SELECTED
       ↓
    PLAYING
       ↕
    PAUSED
       ↓
    EMPTY / EXPIRED
       ↓
    DESTROYED

The creator is the host.

Only the creator can:

- select the movie
- select the TV episode
- change the currently watched media
- optionally end/dissolve the room

Participants can:

- join
- leave
- receive playback state
- send playback events according to the permission model
- chat

When the creator leaves, define and implement a clear policy.

Prefer:

    creator leaves
        ↓
    room ownership transferred to another participant

rather than immediately destroying the room.

However, make this behavior explicit and configurable in the architecture.

If no participants remain, destroy the room after a short configurable expiration period.

Rooms should normally be ephemeral.

---

# 7. Room creation

Provide an HTTP API and/or WebSocket command for:

    POST /api/v1/rooms

The response should contain:

- room ID
- creator identity
- join information
- room state

Room IDs should be:

- unpredictable
- short enough to share comfortably
- not sequential database IDs

Do not use predictable integer room IDs.

Consider something like a cryptographically secure random identifier.

---

# 8. Joining rooms

Users should be able to join using a room ID.

Example:

    POST /api/v1/rooms/:room_id/join

or an equivalent WebSocket operation.

Joining must authenticate the user.

On successful join:

1. add participant
2. broadcast participant_joined
3. send complete current room state to the new participant
4. provide the current playback position
5. provide current media
6. provide playback status
7. provide playback rate
8. provide host identity

The new participant should be able to immediately synchronize with the room.

---

# 9. Host media selection

Only the host can choose what is being watched.

FlickSync should NOT download, inspect, stream, or proxy the media.

Instead, the host sends a media reference.

The media reference should be designed to work with both Jellyfin and Plex.

Do not assume that a Jellyfin ID and Plex ID are interchangeable.

A media reference should therefore contain enough information for Flick to resolve the media through its configured media server.

For example, conceptually:

    provider: jellyfin | plex
    server_id: ...
    media_id: ...
    media_type: movie | episode
    season_id: optional
    episode_id: optional

Use the actual Flick data model if one already exists.

The important point:

FlickSync coordinates identity.
Flick resolves and plays the media.

---

# 10. Playback synchronization

This is the most important part of the project.

Do NOT implement synchronization as:

    "user A sends current position every second"

That will drift and feel bad.

Instead, model playback as a state with a reference timestamp.

Conceptually:

    position_at_reference
    playback_state
    playback_rate
    server_timestamp

For example:

    {
        "position": 123.42,
        "state": "playing",
        "rate": 1.0,
        "server_time": 1234567890
    }

If playback is active, clients should be able to calculate:

    current_position =
        position_at_reference
        + elapsed_time * playback_rate

The server should use a monotonic clock internally whenever measuring elapsed durations.

Wall-clock timestamps may be used for communication/debugging, but do not rely on system clock adjustments for synchronization calculations.

---

# 11. Authoritative synchronization

The server is authoritative for room playback state.

Clients are responsible for:

- local media playback
- rendering
- local buffering
- communicating playback observations

The server is responsible for:

- determining the canonical room state
- accepting valid playback commands
- broadcasting state changes
- detecting meaningful drift
- requesting corrections

Do not make the server constantly stream the playback position.

State changes should be event-driven.

Periodic synchronization messages may be used as a lightweight heartbeat/correction mechanism.

---

# 12. Drift correction

The system must account for the fact that:

- network latency exists
- clients have different hardware
- media buffering differs
- playback clocks drift
- seeking is imperfect

Implement a drift correction strategy.

Example conceptual policy:

    drift < 100 ms
        do nothing

    100–500 ms
        gradually adjust playback rate

    500–1500 ms
        stronger rate correction

    > 1500 ms
        hard seek to canonical position

These are starting points, NOT hard requirements.

Choose sensible thresholds after understanding how the client can expose playback state.

Avoid constant hard seeks.

The experience should feel smooth.

The goal is not mathematical perfection at every millisecond.

The goal is that participants perceive playback as synchronized.

Make thresholds configurable.

---

# 13. Playback events

Define a versioned WebSocket protocol.

At minimum support:

    room_state
    participant_joined
    participant_left

    media_selected

    playback_play
    playback_pause
    playback_seek
    playback_rate_changed

    sync_state
    sync_request
    sync_correction

    room_updated
    room_closed

    chat_message
    chat_history

    error

The protocol must be explicitly documented.

Use structured JSON initially.

Example:

{
    "type": "playback_pause",
    "payload": {
        "position": 123.42
    }
}

Use a protocol version.

For example:

    "protocol_version": 1

Design the protocol so it can evolve without breaking older clients unnecessarily.

---

# 14. Play / pause behavior

If the host pauses:

    host
      ↓
    server
      ↓
    all participants pause

If a participant pauses locally, determine whether this should:

A) affect everyone
B) only report drift
C) require host permission

Design this as a clear permission model.

For the initial implementation, prefer:

- host controls media selection
- playback controls may be shared
- all participants can request playback changes
- server resolves the resulting canonical state

But ensure this is configurable so Flick can later expose:

    Host only
    Everyone

Do not assume that "everyone can control playback" is always desirable.

---

# 15. Seek behavior

Seeking should be treated as a room-level state change.

Example:

Participant requests:

    seek -> 540.25 seconds

Server validates it and broadcasts:

    playback_seek
    position = 540.25
    server_timestamp = ...

All clients seek to the same canonical position.

Avoid race conditions such as:

    A seeks to 500
    B seeks to 700
    C seeks to 600

Implement a deterministic command ordering mechanism.

Use server-side sequence numbers or another suitable mechanism.

Every room playback state change should have a monotonically increasing sequence number.

Example:

    sequence = 42

This helps clients ignore stale messages.

---

# 16. Playback rate

Support playback rates.

For example:

    0.5
    0.75
    1.0
    1.25
    1.5
    2.0

Do not hard-code the list into the server unless necessary.

Validate rates against sane boundaries.

---

# 17. Client synchronization handshake

When a client connects:

1. authenticate
2. join room
3. receive canonical room state
4. estimate network latency / round-trip time
5. calculate local target position
6. begin playback or pause accordingly
7. participate in periodic synchronization

Implement a ping/pong mechanism.

Example:

    client -> ping(timestamp)
    server -> pong(timestamp)

Use this to estimate RTT.

Where appropriate, compensate for network latency when calculating the target position.

Do not assume all clients have the same RTT.

---

# 18. Reconnection

WebSocket connections WILL disconnect.

Design for it.

A participant should be able to:

    disconnect
       ↓
    reconnect
       ↓
    authenticate
       ↓
    rejoin existing room
       ↓
    receive canonical state
       ↓
    resynchronize

Do not destroy a participant's identity simply because their WebSocket disappeared for a few seconds.

Use a participant/session identifier.

Define a reasonable reconnection grace period.

---

# 19. Presence

Track:

- connected
- disconnected
- reconnecting

Broadcast presence changes.

Do not treat a temporary WebSocket failure as an immediate permanent leave.

---

# 20. Chat

Implement an optional room chat.

Requirements:

- text messages
- sender identity
- timestamp
- message ID
- room association

Do not build a complete Discord clone.

Messages should be lightweight.

Consider:

- maximum message length
- rate limiting
- flood protection
- basic sanitization
- maximum history size

Chat history can initially exist only in memory.

If persistent history is implemented, make it optional.

Do not store chat permanently by default.

---

# 21. Authentication

Do NOT create an independent user/password system.

FlickSync should trust authentication performed by Flick Server.

Design an authentication mechanism suitable for server-to-server/client integration.

Preferred architecture:

    Flick Client
         ↓
    Flick Server
         ↓
    authenticated token
         ↓
    FlickSync

The exact mechanism should be compatible with the existing Flick architecture if available.

If Flick currently has no authentication protocol, implement a documented signed-token mechanism.

Potential model:

    Flick Server issues a signed JWT or similar token
    FlickSync validates the signature
    FlickSync extracts:
        user_id
        server_id
        permissions
        expiration

Do NOT call an external authentication server for every WebSocket message.

Authentication must be locally verifiable where possible.

Tokens must expire.

Do not put secrets in client code.

Document key rotation.

---

# 22. Server trust model

FlickSync should distinguish:

- Flick Server
- Flick client
- room participant

Do not blindly trust client-provided server identity.

A malicious client should not be able to impersonate another Flick Server.

The authentication design must bind the token to the authorized Flick Server instance.

---

# 23. Security

Treat all client input as untrusted.

Implement:

- authentication
- authorization
- message validation
- payload size limits
- WebSocket frame limits
- rate limiting
- chat flood protection
- room creation rate limits
- connection limits
- timeout handling
- malformed JSON handling
- graceful protocol errors

Do not allow:

- arbitrary server-side URLs
- filesystem paths
- shell commands
- arbitrary proxying
- arbitrary HTTP fetching

FlickSync should not become an SSRF or open proxy.

---

# 24. Room authorization

Users must not be able to:

- modify rooms they are not members of
- select media if they are not host
- send messages to arbitrary rooms
- impersonate another participant
- change playback state after leaving
- access private room state without authorization

Validate every command server-side.

Never rely on the client UI for authorization.

---

# 25. API

Implement a minimal HTTP API.

At minimum:

    GET /health
    GET /ready

    POST /api/v1/rooms
    GET  /api/v1/rooms/:room_id
    POST /api/v1/rooms/:room_id/join
    POST /api/v1/rooms/:room_id/leave

WebSocket:

    GET /api/v1/rooms/:room_id/ws

Exact endpoints may be changed if a cleaner design emerges.

Provide OpenAPI documentation if practical.

Do not overbuild REST endpoints for operations that are naturally WebSocket events.

---

# 26. Configuration

Support environment variables.

Example:

    FLICKSYNC_HOST=0.0.0.0
    FLICKSYNC_PORT=xxxx

    FLICKSYNC_LOG_LEVEL=info

    FLICKSYNC_MAX_ROOM_SIZE=...
    FLICKSYNC_ROOM_TIMEOUT=...

    FLICKSYNC_SYNC_DRIFT_SOFT=...
    FLICKSYNC_SYNC_DRIFT_HARD=...

    FLICKSYNC_AUTH_...

Do not hard-code operational settings.

Provide a documented `.env.example`.

---

# 27. Persistence

Do NOT persist everything.

Room state should primarily live in memory.

Potential persistent data:

- server configuration
- optional room metadata
- optional audit information

Do not store:

- media files
- playback streams
- user passwords
- unnecessary personal information

If persistence is not required for the first version, omit the database entirely.

The service should be capable of running with:

    Rust binary
    ↓
    memory
    ↓
    WebSockets

This is desirable.

---

# 28. Scaling

Design the code so a future architecture could become:

    Load Balancer
        ↓
    FlickSync instance A
    FlickSync instance B
    FlickSync instance C
        ↓
       Redis

But DO NOT implement Redis just for the sake of it.

Document what would be required to support multi-instance rooms.

In particular, explain:

- room ownership
- WebSocket routing
- event broadcasting
- distributed room state
- participant presence

The initial implementation can remain single-instance.

---

# 29. Observability

Use structured logging with `tracing`.

Useful events:

- server startup
- room creation
- room destruction
- participant join
- participant leave
- authentication failures
- WebSocket disconnect
- synchronization corrections
- malformed messages
- rate-limit violations

Never log:

- authentication tokens
- passwords
- private chat contents by default
- sensitive user information

Include useful IDs:

    room_id
    participant_id
    server_id

where appropriate.

---

# 30. Metrics

Design for metrics but do not introduce Prometheus unless useful.

Useful metrics include:

- active rooms
- active participants
- WebSocket connections
- room creation rate
- message rate
- synchronization corrections
- average RTT
- authentication failures

If a metrics endpoint is implemented, keep it optional.

---

# 31. Error handling

Use typed Rust errors.

The server must never panic because of malformed client input.

Return structured protocol errors.

Example:

{
    "type": "error",
    "payload": {
        "code": "NOT_HOST",
        "message": "Only the room host can select media."
    }
}

Do not expose internal stack traces to clients.

---

# 32. Testing

Testing is important.

Implement:

### Unit tests

For:

- room lifecycle
- permissions
- playback state transitions
- sequence numbers
- drift calculation
- playback position calculation
- rate changes
- host transfer
- reconnection
- room expiration

### Integration tests

Test:

    create room
    ↓
    connect participant A
    ↓
    connect participant B
    ↓
    select media
    ↓
    play
    ↓
    pause
    ↓
    seek
    ↓
    reconnect
    ↓
    leave
    ↓
    room destruction

### Protocol tests

Test malformed:

- JSON
- unknown event types
- invalid sequence numbers
- invalid positions
- invalid playback rates
- oversized messages
- unauthorized commands

### Concurrency tests

Test simultaneous:

- joins
- leaves
- seeks
- play/pause
- disconnect/reconnect

Pay particular attention to race conditions.

---

# 33. Time synchronization tests

Create deterministic tests for synchronization.

For example:

Given:

    canonical position = 100.0s
    canonical rate = 1.0
    reference time = T

and a client receives the state at:

    T + 500ms

the calculated target should account for the elapsed time and, where appropriate, estimated network delay.

Test:

- 10 ms RTT
- 50 ms RTT
- 100 ms RTT
- 300 ms RTT
- temporary packet delay
- reconnect after 5 seconds
- playback rate 1.5x

Do not make tests depend on actual wall-clock timing where avoidable.

Use injectable clocks/time sources where useful.

---

# 34. Protocol documentation

Create:

    docs/protocol.md

Document:

- connection flow
- authentication
- room lifecycle
- every WebSocket event
- payload schemas
- error codes
- sequence numbers
- synchronization model
- reconnection behavior
- host behavior
- permissions

The documentation should be understandable to the Flick client developer.

---

# 35. Integration contract with Flick

Create:

    docs/flick-integration.md

Explain exactly what Flick must implement.

For example:

1. configure FlickSync server
2. authenticate
3. create/join room
4. receive room state
5. resolve media through Jellyfin/Plex
6. start playback
7. report local playback state
8. apply synchronization corrections
9. handle disconnect/reconnect
10. display participants
11. display chat

Be explicit about which responsibilities belong to FlickSync and which belong to Flick.

---

# 36. Docker

Provide a production-ready Dockerfile.

Requirements:

- multi-stage build
- minimal runtime image
- non-root user
- configurable port
- health check
- no unnecessary packages

Provide:

    docker-compose.yml

with the minimum required configuration.

Do not require a database or Redis unless actually necessary.

---

# 37. Deployment

The service should be easy to deploy on:

- VPS
- Docker
- Dokploy
- Coolify
- Docker Compose
- bare-metal Linux

Document the simplest deployment path.

The service should work behind:

- Nginx
- Traefik
- Cloudflare proxy where WebSockets are supported

Make sure WebSocket upgrade behavior is documented.

---

# 38. CORS / networking

Implement CORS carefully.

Do NOT use:

    Access-Control-Allow-Origin: *

by default if authentication is involved.

Make allowed origins configurable.

Remember that native desktop clients may not behave exactly like browsers.

Do not introduce browser-only assumptions into the protocol.

---

# 39. Performance goals

The service should be capable of handling many small concurrent WebSocket connections without unnecessary overhead.

Optimize only where justified.

Priorities:

1. correctness
2. synchronization quality
3. reliability
4. security
5. simplicity
6. performance

Do not prematurely optimize.

---

# 40. UX implications

Although you are building the backend, always consider the resulting Flick UX.

The user should experience:

    "Create room"
    ↓
    share room
    ↓
    friends join
    ↓
    choose movie
    ↓
    watch together

The synchronization should feel automatic.

Users should not need to understand:

- WebSockets
- clocks
- RTT
- drift correction
- server timestamps

Expose technical information only where useful for debugging.

---

# 41. Important design decision: synchronization authority

Think carefully about this before implementing.

FlickSync should not attempt to continuously dictate playback every few milliseconds.

Use an authoritative state + event-driven correction model.

A good conceptual model is:

    Room State

    media
    position
    playback_state
    playback_rate
    reference_timestamp
    sequence

Clients derive their expected position locally.

FlickSync periodically verifies that clients remain synchronized.

This minimizes network traffic and makes the system scalable.

---

# 42. Important design decision: media ownership

FlickSync must never assume it can access the user's Jellyfin/Plex server.

FlickSync only knows:

    "this room is watching media X"

Flick resolves media X locally through its own configured Jellyfin/Plex connection.

This separation is fundamental.

Never implement server-side media fetching.

---

# 43. Important design decision: no permanent account system

Do not build:

    username
    password
    email
    password reset
    registration

unless the existing Flick architecture absolutely requires it.

Identity belongs to Flick/Flick Server.

FlickSync is a coordination service.

---

# 44. Code quality

Follow idiomatic Rust.

Use:

- cargo fmt
- cargo clippy
- cargo test

Avoid:

- giant files
- giant match statements containing the entire application
- unnecessary trait abstractions
- global mutable state
- unsafe code unless absolutely necessary
- blocking operations inside async code

Keep the domain logic independently testable.

---

# 45. Development workflow

Work incrementally.

Recommended order:

PHASE 1
    Repository inspection
    Architecture
    protocol design
    documentation

PHASE 2
    Axum server
    health endpoint
    configuration
    logging

PHASE 3
    authentication foundation

PHASE 4
    room manager
    create/join/leave
    participant lifecycle

PHASE 5
    WebSocket protocol

PHASE 6
    playback state machine

PHASE 7
    synchronization engine

PHASE 8
    reconnection

PHASE 9
    chat

PHASE 10
    rate limiting/security hardening

PHASE 11
    tests

PHASE 12
    Docker/deployment

Do not attempt to write the entire project in one giant file or one giant implementation step.

After each major phase, run tests and fix regressions.

---

# 46. Definition of done

The project is considered complete for V1 when:

- FlickSync starts successfully
- health checks work
- authentication works
- rooms can be created
- participants can join
- participants can leave
- host can select media
- all participants receive media state
- play synchronizes
- pause synchronizes
- seek synchronizes
- playback rate synchronizes
- drift is corrected
- reconnect works
- host transfer works
- room expiration works
- chat works
- invalid requests are rejected
- unauthorized actions are rejected
- malformed WebSocket messages do not crash the server
- concurrent room operations are safe
- tests cover core synchronization logic
- Docker deployment works
- protocol documentation exists
- Flick integration documentation exists

---

# 47. Before coding

First produce a concise architecture assessment based on the repository.

Include:

1. existing relevant architecture
2. proposed FlickSync architecture
3. protocol overview
4. authentication model
5. synchronization model
6. room lifecycle
7. important risks
8. assumptions
9. implementation phases

Then begin implementation.

Do not ask for confirmation unless you encounter a decision that genuinely cannot be resolved from the repository or the requirements.

If a reasonable default exists, choose it, implement it, and document the decision.

---

# 48. Final engineering principle

FlickSync should feel like a small, boring, reliable piece of infrastructure.

Do not over-engineer it.

Do not turn it into a social network.

Do not turn it into a media server.

Do not turn it into a generic backend framework.

Its job is simple:

    connect people
        ↓
    create a shared room
        ↓
    agree on what to watch
        ↓
    synchronize playback
        ↓
    keep everyone together
        ↓
    get out of the way

Make that experience extremely reliable.
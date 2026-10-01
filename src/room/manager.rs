//! Owns all rooms, serialises access to each of them and delivers messages.
//!
//! Concurrency model: the room map is behind an `RwLock` that is only held to
//! look up / insert / remove an `Arc`. Each room has its own `Mutex`, so rooms
//! never contend with one another and every operation on one room is totally
//! ordered (this is what gives deterministic command ordering). No lock is held
//! across an `.await` and delivery uses non-blocking `try_send`.
//! Lock order is always map -> room, and a room lock is never taken while a
//! map guard is held.
//!
//! Scaling note: everything that would have to change for multi-instance
//! operation (room ownership, routing, fan-out) sits behind this type; see
//! `docs/scaling.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use rand::RngExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::{Outcome, ParticipantInfo, Room, RoomConfig, Target};
use crate::auth::{Identity, PERM_CHAT, PERM_CREATE_ROOM, PERM_JOIN_ROOM};
use crate::errors::{Error, ErrorCode, Result};
use crate::metrics::Metrics;
use crate::protocol::{ClientMessage, ControlMode, RoomView};
use crate::ratelimit::TokenBucket;
use crate::sync::clock::{Clock, Time};

const ROOM_ID_LEN: usize = 12;
/// Crockford base32 (no I, L, O, U): easy to read aloud and to type.
const ROOM_ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone)]
pub struct ManagerConfig {
    pub room: Arc<RoomConfig>,
    pub max_rooms: usize,
    /// Rooms a single user may create per minute (also the burst size).
    pub create_per_minute: f64,
    /// Capacity of each connection's outbound queue.
    pub outbound_buffer: usize,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            room: Arc::new(RoomConfig::default()),
            max_rooms: 10_000,
            create_per_minute: 6.0,
            outbound_buffer: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// The same participant connected again.
    Replaced,
    /// The participant left or was removed from the room.
    Removed,
    RoomClosed,
    /// The connection could not keep up with outbound traffic.
    SlowConsumer,
    Shutdown,
}

/// What the manager pushes to a connection task.
#[derive(Debug, Clone)]
pub enum Outbound {
    Text(Arc<str>),
    Close(CloseReason),
}

pub struct Attachment {
    pub conn_id: u64,
    pub rx: mpsc::Receiver<Outbound>,
}

pub struct CreatedRoom {
    pub room: RoomView,
    pub participant_id: String,
}

#[derive(Debug, Default, Clone)]
pub struct CreateOptions {
    pub control_mode: Option<ControlMode>,
    pub chat_enabled: Option<bool>,
}

struct Conn {
    id: u64,
    tx: mpsc::Sender<Outbound>,
}

struct RoomEntry {
    room: Room,
    conns: HashMap<String, Conn>,
}

pub struct RoomManager {
    cfg: ManagerConfig,
    clock: Arc<dyn Clock>,
    metrics: Arc<Metrics>,
    rooms: RwLock<HashMap<String, Arc<Mutex<RoomEntry>>>>,
    create_limits: Mutex<HashMap<String, TokenBucket>>,
    next_conn_id: AtomicU64,
    accepting: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means another thread panicked; the data is plain
    // state we can keep serving rather than cascading the panic.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn not_found() -> Error {
    Error::new(ErrorCode::RoomNotFound, "room not found")
}

/// Canonical form of a room id: upper case, no separators. `None` if malformed.
pub fn normalize_room_id(input: &str) -> Option<String> {
    let id: String = input
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (id.len() == ROOM_ID_LEN && id.bytes().all(|b| ROOM_ID_ALPHABET.contains(&b))).then_some(id)
}

/// Human-friendly grouping, e.g. `K7M2-Q9XP-4TWB`.
pub fn share_code(room_id: &str) -> String {
    room_id
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

fn generate_room_id() -> String {
    // `rand::rng()` is a CSPRNG seeded from the OS: ids are unpredictable (60 bits).
    let mut rng = rand::rng();
    (0..ROOM_ID_LEN)
        .map(|_| ROOM_ID_ALPHABET[rng.random_range(0..ROOM_ID_ALPHABET.len())] as char)
        .collect()
}

fn participant_info(identity: &Identity) -> ParticipantInfo {
    ParticipantInfo {
        id: identity.user_id.clone(),
        display_name: identity.display_name.clone(),
        can_chat: identity.permits(PERM_CHAT),
    }
}

impl RoomManager {
    pub fn new(cfg: ManagerConfig, clock: Arc<dyn Clock>, metrics: Arc<Metrics>) -> Self {
        Self {
            cfg,
            clock,
            metrics,
            rooms: RwLock::new(HashMap::new()),
            create_limits: Mutex::new(HashMap::new()),
            next_conn_id: AtomicU64::new(1),
            accepting: AtomicBool::new(true),
        }
    }

    pub fn config(&self) -> &ManagerConfig {
        &self.cfg
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    pub fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    pub fn room_count(&self) -> usize {
        self.rooms.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn entry(&self, room_id: &str) -> Result<(String, Arc<Mutex<RoomEntry>>)> {
        let id = normalize_room_id(room_id).ok_or_else(not_found)?;
        let map = self.rooms.read().unwrap_or_else(|e| e.into_inner());
        let e = map.get(&id).cloned().ok_or_else(not_found)?;
        Ok((id, e))
    }

    /// Rooms are scoped to the Flick Server of their creator. Other servers' users
    /// get a plain "not found" so room existence is not observable across servers.
    fn check_scope(entry: &RoomEntry, identity: &Identity) -> Result<()> {
        if entry.room.server_id() == identity.server_id {
            Ok(())
        } else {
            Err(not_found())
        }
    }

    // ------------------------------------------------------------ HTTP-level operations

    pub fn create_room(&self, identity: &Identity, opts: CreateOptions) -> Result<CreatedRoom> {
        identity.require(PERM_CREATE_ROOM)?;
        if !self.is_accepting() {
            return Err(Error::new(ErrorCode::Internal, "server is shutting down"));
        }
        let now = self.clock.now();
        {
            let per_min = self.cfg.create_per_minute;
            let mut limits = lock(&self.create_limits);
            let key = format!("{}/{}", identity.server_id, identity.user_id);
            let bucket = limits
                .entry(key)
                .or_insert_with(|| TokenBucket::new(per_min.max(1.0), per_min / 60.0, now.mono_ms));
            if !bucket.try_acquire(now.mono_ms) {
                self.metrics.rate_limited_total.inc();
                warn!(server_id = %identity.server_id, participant_id = %identity.user_id, "room creation rate limit hit");
                return Err(Error::new(
                    ErrorCode::RateLimited,
                    "too many rooms created, try again later",
                ));
            }
        }

        let room_cfg = self.cfg.room.clone();
        let mut map = self.rooms.write().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.cfg.max_rooms {
            return Err(Error::new(
                ErrorCode::TooManyRooms,
                "server room capacity reached",
            ));
        }
        let id = loop {
            let candidate = generate_room_id();
            if !map.contains_key(&candidate) {
                break candidate;
            }
        };
        let room = Room::new(
            id.clone(),
            identity.server_id.clone(),
            participant_info(identity),
            opts.control_mode.unwrap_or(room_cfg.default_control_mode),
            opts.chat_enabled.unwrap_or(room_cfg.chat_enabled),
            room_cfg,
            now,
        );
        let view = room.view(now);
        map.insert(
            id.clone(),
            Arc::new(Mutex::new(RoomEntry {
                room,
                conns: HashMap::new(),
            })),
        );
        self.metrics.rooms_active.set(map.len() as u64);
        drop(map);
        self.metrics.rooms_created_total.inc();
        info!(room_id = %id, server_id = %identity.server_id, participant_id = %identity.user_id, "room created");
        Ok(CreatedRoom {
            room: view,
            participant_id: identity.user_id.clone(),
        })
    }

    pub fn join(&self, room_id: &str, identity: &Identity) -> Result<RoomView> {
        identity.require(PERM_JOIN_ROOM)?;
        let (id, entry) = self.entry(room_id)?;
        let now = self.clock.now();
        let mut g = lock(&entry);
        Self::check_scope(&g, identity)?;
        let out = g.room.join(participant_info(identity), now)?;
        let view = g.room.view(now);
        let closed = self.dispatch(&mut g, now, out);
        drop(g);
        self.finish(&id, &entry, closed);
        Ok(view)
    }

    /// Room state for a member.
    pub fn get_room(&self, room_id: &str, identity: &Identity) -> Result<RoomView> {
        let (_, entry) = self.entry(room_id)?;
        let g = lock(&entry);
        Self::check_scope(&g, identity)?;
        if !g.room.has_participant(&identity.user_id) {
            return Err(Error::new(
                ErrorCode::NotMember,
                "not a member of this room",
            ));
        }
        Ok(g.room.view(self.clock.now()))
    }

    pub fn leave(&self, room_id: &str, identity: &Identity) -> Result<()> {
        let (id, entry) = self.entry(room_id)?;
        let now = self.clock.now();
        let mut g = lock(&entry);
        Self::check_scope(&g, identity)?;
        let out = g.room.leave(&identity.user_id, now)?;
        let closed = self.dispatch(&mut g, now, out);
        drop(g);
        self.finish(&id, &entry, closed);
        Ok(())
    }

    // ------------------------------------------------------------ WebSocket-level operations

    /// Cheap pre-upgrade check so the client gets a proper HTTP error.
    pub fn can_attach(&self, room_id: &str, identity: &Identity) -> Result<()> {
        identity.require(PERM_JOIN_ROOM)?;
        if !self.is_accepting() {
            return Err(Error::new(
                ErrorCode::TooManyConnections,
                "server is shutting down",
            ));
        }
        let (_, entry) = self.entry(room_id)?;
        let g = lock(&entry);
        Self::check_scope(&g, identity)?;
        if g.room.is_closed() {
            return Err(Error::new(ErrorCode::RoomClosed, "this room is closed"));
        }
        if !g.room.has_participant(&identity.user_id) && g.room.is_full() {
            return Err(Error::new(ErrorCode::RoomFull, "this room is full"));
        }
        Ok(())
    }

    /// Attach a socket to the room. Joins implicitly when the user is not yet a member,
    /// and takes over the seat if the participant is already connected elsewhere.
    pub fn attach(&self, room_id: &str, identity: &Identity) -> Result<Attachment> {
        identity.require(PERM_JOIN_ROOM)?;
        let (id, entry) = self.entry(room_id)?;
        let now = self.clock.now();
        let mut g = lock(&entry);
        Self::check_scope(&g, identity)?;
        let pid = identity.user_id.clone();

        let mut out = g.room.join(participant_info(identity), now)?;

        let (tx, rx) = mpsc::channel(self.cfg.outbound_buffer);
        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed);
        if let Some(old) = g.conns.insert(pid.clone(), Conn { id: conn_id, tx }) {
            debug!(room_id = %id, participant_id = %pid, "connection replaced by a newer one");
            let _ = old.tx.try_send(Outbound::Close(CloseReason::Replaced));
        }

        let connect = g.room.connect(&pid, now)?;
        out.deliveries.extend(connect.deliveries);
        let closed = self.dispatch(&mut g, now, out);
        drop(g);
        self.finish(&id, &entry, closed);
        Ok(Attachment { conn_id, rx })
    }

    pub fn detach(&self, room_id: &str, pid: &str, conn_id: u64) {
        let Ok((id, entry)) = self.entry(room_id) else {
            return;
        };
        let now = self.clock.now();
        let mut g = lock(&entry);
        match g.conns.get(pid) {
            Some(c) if c.id == conn_id => {
                g.conns.remove(pid);
            }
            // Already replaced by a newer connection (or removed): nothing to do.
            _ => return,
        }
        let out = g.room.disconnect(pid, now);
        let closed = self.dispatch(&mut g, now, out);
        drop(g);
        self.finish(&id, &entry, closed);
    }

    /// Apply a client message. Errors are returned for the caller to report to that client only.
    pub fn handle_message(
        &self,
        room_id: &str,
        pid: &str,
        conn_id: u64,
        msg: ClientMessage,
    ) -> Result<()> {
        let (id, entry) = self.entry(room_id)?;
        let now = self.clock.now();
        let mut g = lock(&entry);
        if g.conns.get(pid).map(|c| c.id) != Some(conn_id) {
            return Err(Error::new(
                ErrorCode::SessionReplaced,
                "this connection has been replaced or removed",
            ));
        }
        let out = g.room.handle(pid, msg, now);
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                if e.code == ErrorCode::RateLimited {
                    self.metrics.rate_limited_total.inc();
                    debug!(room_id = %id, participant_id = %pid, "message rate limit hit");
                }
                return Err(e);
            }
        };
        let closed = self.dispatch(&mut g, now, out);
        drop(g);
        self.finish(&id, &entry, closed);
        Ok(())
    }

    // ------------------------------------------------------------ maintenance

    /// Run time-driven maintenance on every room. Called periodically by the server.
    pub fn sweep(&self) {
        let entries: Vec<(String, Arc<Mutex<RoomEntry>>)> = {
            let map = self.rooms.read().unwrap_or_else(|e| e.into_inner());
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut participants = 0u64;
        let (mut rtt_sum, mut rtt_n) = (0.0f64, 0u32);
        for (id, entry) in entries {
            let now = self.clock.now();
            let mut g = lock(&entry);
            let out = g.room.tick(now);
            let closed = self.dispatch(&mut g, now, out);
            participants += g.room.participant_count() as u64;
            let (s, n) = g.room.rtt_stats();
            rtt_sum += s;
            rtt_n += n;
            drop(g);
            self.finish(&id, &entry, closed);
        }
        self.metrics.participants_active.set(participants);
        let avg = if rtt_n > 0 {
            rtt_sum / rtt_n as f64
        } else {
            0.0
        };
        self.metrics.rtt_avg_us.set((avg * 1000.0) as u64);

        let now = self.clock.now();
        lock(&self.create_limits).retain(|_, b| !b.is_idle(now.mono_ms));
    }

    /// Stop accepting new work and close every room and connection.
    pub fn shutdown(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        let entries: Vec<_> = {
            let mut map = self.rooms.write().unwrap_or_else(|e| e.into_inner());
            let v = map.drain().map(|(_, v)| v).collect();
            self.metrics.rooms_active.set(0);
            v
        };
        for entry in entries {
            let now = self.clock.now();
            let mut g = lock(&entry);
            let out = g.room.shutdown();
            self.dispatch(&mut g, now, out);
        }
    }

    // ------------------------------------------------------------ internals

    fn finish(&self, id: &str, entry: &Arc<Mutex<RoomEntry>>, closed: bool) {
        if !closed {
            return;
        }
        let mut map = self.rooms.write().unwrap_or_else(|e| e.into_inner());
        if map.get(id).is_some_and(|e| Arc::ptr_eq(e, entry)) {
            map.remove(id);
            self.metrics.rooms_active.set(map.len() as u64);
            self.metrics.rooms_destroyed_total.inc();
            info!(room_id = %id, "room destroyed");
        }
    }

    /// Deliver an outcome. Returns true when the room is over.
    ///
    /// Connections that cannot take more messages (queue full or gone) are
    /// detached, which marks the participant as reconnecting and may generate
    /// further presence messages, hence the loop.
    fn dispatch(&self, entry: &mut RoomEntry, now: Time, first: Outcome) -> bool {
        let mut closed = false;
        let mut pending = Some(first);
        while let Some(mut outcome) = pending.take() {
            closed |= outcome.closed;
            self.metrics
                .sync_corrections_total
                .add(outcome.corrections as u64);

            let mut dead: Vec<String> = Vec::new();
            for delivery in outcome.deliveries.drain(..) {
                let text: Arc<str> = delivery.message.to_json().into();
                let send = |conn: &Conn| conn.tx.try_send(Outbound::Text(text.clone())).is_ok();
                match &delivery.target {
                    Target::All => {
                        for (pid, c) in &entry.conns {
                            if !send(c) {
                                dead.push(pid.clone());
                            }
                        }
                    }
                    Target::AllExcept(skip) => {
                        for (pid, c) in &entry.conns {
                            if pid != skip && !send(c) {
                                dead.push(pid.clone());
                            }
                        }
                    }
                    Target::Only(pid) => {
                        if let Some(c) = entry.conns.get(pid)
                            && !send(c)
                        {
                            dead.push(pid.clone());
                        }
                    }
                }
            }

            for pid in outcome.removed.drain(..) {
                if let Some(c) = entry.conns.remove(&pid) {
                    let _ = c.tx.try_send(Outbound::Close(CloseReason::Removed));
                }
            }

            if outcome.closed {
                for (_, c) in entry.conns.drain() {
                    let _ = c.tx.try_send(Outbound::Close(CloseReason::RoomClosed));
                }
                break;
            }

            let mut follow_up = Outcome::default();
            dead.sort();
            dead.dedup();
            for pid in dead {
                if let Some(c) = entry.conns.remove(&pid) {
                    warn!(room_id = %entry.room.id(), participant_id = %pid, "dropping slow or dead connection");
                    let _ = c.tx.try_send(Outbound::Close(CloseReason::SlowConsumer));
                    let o = entry.room.disconnect(&pid, now);
                    follow_up.deliveries.extend(o.deliveries);
                }
            }
            if !follow_up.deliveries.is_empty() {
                pending = Some(follow_up);
            }
        }
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_ids_are_random_and_well_formed() {
        let a = generate_room_id();
        let b = generate_room_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), ROOM_ID_LEN);
        assert_eq!(normalize_room_id(&a), Some(a.clone()));
        assert_eq!(normalize_room_id(&a.to_lowercase()), Some(a.clone()));
        assert_eq!(normalize_room_id(&share_code(&a)), Some(a));
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad in [
            "",
            "short",
            "../../etc/passwd",
            "ILOU00000000",
            "0123456789ABC",
            "01234567 9AB",
        ] {
            assert_eq!(normalize_room_id(bad), None, "{bad}");
        }
    }
}

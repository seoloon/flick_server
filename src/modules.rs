//! Runtime slot of a module: what is running, if anything.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::settings::store::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    FlickSync,
    FlickDd,
}

impl ModuleId {
    pub const ALL: [ModuleId; 2] = [ModuleId::FlickSync, ModuleId::FlickDd];

    pub fn id(self) -> &'static str {
        self.scope().id()
    }

    pub fn from_id(id: &str) -> Option<ModuleId> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    pub fn scope(self) -> Scope {
        match self {
            ModuleId::FlickSync => Scope::FlickSync,
            ModuleId::FlickDd => Scope::FlickDd,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Running<T> {
    pub rt: Arc<T>,
    /// Fingerprint of the module's settings when it started.
    pub fingerprint: u64,
    pub since_ms: u64,
}

impl<T> Running<T> {
    pub fn new(rt: Arc<T>, fingerprint: u64) -> Self {
        Self {
            rt,
            fingerprint,
            since_ms: now_ms(),
        }
    }
}

pub enum SlotState<T> {
    Stopped,
    Running(Running<T>),
    Failed(String),
}

/// One module's runtime. Handlers call [`Slot::get`] and keep the `Arc` for the whole request,
/// so a reload can never free memory under them.
pub struct Slot<T> {
    state: RwLock<SlotState<T>>,
    gate: Mutex<()>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            state: RwLock::new(SlotState::Stopped),
            gate: Mutex::new(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleStatus {
    pub id: &'static str,
    /// `stopped`, `running` or `failed`.
    pub state: &'static str,
    /// Why the module failed to start.
    pub message: Option<String>,
    /// The persisted on/off switch.
    pub enabled: bool,
    /// Running with older settings than the ones now in force.
    pub pending_reload: bool,
    /// Start time (ms since the epoch) while running.
    pub since: Option<u64>,
}

impl<T> Slot<T> {
    pub fn get(&self) -> Option<Arc<T>> {
        match &*self.state.read().unwrap_or_else(|e| e.into_inner()) {
            SlotState::Running(r) => Some(r.rt.clone()),
            _ => None,
        }
    }

    /// Held for the whole of a start, stop or reload.
    pub fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn replace(&self, new: SlotState<T>) -> SlotState<T> {
        std::mem::replace(
            &mut *self.state.write().unwrap_or_else(|e| e.into_inner()),
            new,
        )
    }

    pub fn status(&self, id: ModuleId, enabled: bool, current_fingerprint: u64) -> ModuleStatus {
        let base = ModuleStatus {
            id: id.id(),
            state: "stopped",
            message: None,
            enabled,
            pending_reload: false,
            since: None,
        };
        match &*self.state.read().unwrap_or_else(|e| e.into_inner()) {
            SlotState::Stopped => base,
            SlotState::Running(r) => ModuleStatus {
                state: "running",
                pending_reload: r.fingerprint != current_fingerprint,
                since: Some(r.since_ms),
                ..base
            },
            SlotState::Failed(m) => ModuleStatus {
                state: "failed",
                message: Some(m.clone()),
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_ids_round_trip() {
        for id in ModuleId::ALL {
            assert_eq!(ModuleId::from_id(id.id()), Some(id));
        }
        assert_eq!(ModuleId::from_id("nope"), None);
        assert_eq!(ModuleId::FlickSync.scope(), Scope::FlickSync);
        assert_eq!(ModuleId::FlickDd.scope(), Scope::FlickDd);
    }

    #[test]
    fn a_slot_goes_stopped_running_failed() {
        let slot: Slot<u32> = Slot::default();
        assert!(slot.get().is_none());
        let s = slot.status(ModuleId::FlickSync, false, 1);
        assert_eq!(
            (s.state, s.enabled, s.pending_reload, s.since),
            ("stopped", false, false, None)
        );

        assert!(matches!(
            slot.replace(SlotState::Running(Running::new(Arc::new(7), 1))),
            SlotState::Stopped
        ));
        assert_eq!(*slot.get().unwrap(), 7);
        let s = slot.status(ModuleId::FlickSync, true, 1);
        assert_eq!((s.state, s.pending_reload), ("running", false));
        assert!(s.since.is_some());

        // The stored settings moved on since the module started.
        assert!(slot.status(ModuleId::FlickSync, true, 2).pending_reload);

        let prev = slot.replace(SlotState::Failed("boom".into()));
        assert!(matches!(prev, SlotState::Running(_)));
        assert!(slot.get().is_none());
        let s = slot.status(ModuleId::FlickDd, true, 2);
        assert_eq!(
            (s.state, s.message.as_deref(), s.pending_reload),
            ("failed", Some("boom"), false)
        );
    }

    #[test]
    fn the_gate_serialises_critical_sections() {
        let slot: Arc<Slot<u32>> = Arc::new(Slot::default());
        let inside = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let overlap = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (slot, inside, overlap) = (slot.clone(), inside.clone(), overlap.clone());
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _g = slot.gate();
                        if inside.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 0 {
                            overlap.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        std::thread::yield_now();
                        inside.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(!overlap.load(std::sync::atomic::Ordering::SeqCst));
    }
}

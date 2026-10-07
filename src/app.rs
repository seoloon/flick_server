//! Application wiring: shared state, module slots, background tasks and the server loop.

use std::net::SocketAddr;
use std::sync::{Arc, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::auth::{AuthConfigError, Authenticator};
use crate::config::{Config, ConfigError, WsConfig};
use crate::dd::DdState;
use crate::errors::{Error, ErrorCode};
use crate::invite::{self, InviteError};
use crate::metrics::Metrics;
use crate::modules::{ModuleId, Running, Slot, SlotState};
use crate::room::RoomManager;
use crate::settings::Settings;
use crate::sync::{Clock, SystemClock};

/// What FlickSync needs while it runs. Dropped (after `RoomManager::shutdown`) when it stops.
pub struct SyncRuntime {
    pub manager: Arc<RoomManager>,
    pub ws: WsConfig,
    pub conn_limit: Arc<Semaphore>,
}

/// Server-wide settings in force (keys, public address, CORS, metrics). Replaced as a whole.
pub struct ServerRuntime {
    pub cfg: Config,
    pub auth: Authenticator,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("cannot load or create the signing key: {0}")]
    Keys(#[from] InviteError),
    #[error("{0}")]
    Auth(#[from] AuthConfigError),
}

/// Applies a new tracing filter to the running subscriber.
pub type LogControl = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

fn build_server(settings: &Settings) -> Result<ServerRuntime, StartError> {
    let mut cfg = settings.config()?;
    invite::ensure_keys(&mut cfg)?;
    let auth = Authenticator::new(&cfg.auth)?;
    Ok(ServerRuntime { cfg, auth })
}

#[derive(Clone)]
pub struct AppState {
    /// Read once at boot: bind address, intervals, body limit. Never swapped.
    pub boot: Arc<Config>,
    pub settings: Arc<Settings>,
    server: Arc<RwLock<Arc<ServerRuntime>>>,
    sync_slot: Arc<Slot<SyncRuntime>>,
    dd_slot: Arc<Slot<DdState>>,
    pub metrics: Arc<Metrics>,
    pub started_at: Instant,
    clock: Arc<dyn Clock>,
    log_control: Arc<RwLock<Option<LogControl>>>,
}

impl AppState {
    pub fn new(settings: Arc<Settings>) -> Result<Self, StartError> {
        Self::with_clock(settings, Arc::new(SystemClock::new()))
    }

    pub fn with_clock(settings: Arc<Settings>, clock: Arc<dyn Clock>) -> Result<Self, StartError> {
        let boot = settings.config()?;
        let server = build_server(&settings)?;
        let state = Self {
            boot: Arc::new(boot),
            settings,
            server: Arc::new(RwLock::new(Arc::new(server))),
            sync_slot: Arc::new(Slot::default()),
            dd_slot: Arc::new(Slot::default()),
            metrics: Arc::new(Metrics::default()),
            started_at: Instant::now(),
            clock,
            log_control: Arc::new(RwLock::new(None)),
        };
        for id in ModuleId::ALL {
            if state.settings.is_enabled(id.scope()) {
                let _gate = state.gate(id);
                state.start_locked(id);
            }
        }
        Ok(state)
    }

    pub fn server(&self) -> Arc<ServerRuntime> {
        self.server
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_log_control(&self, control: LogControl) {
        *self.log_control.write().unwrap_or_else(|e| e.into_inner()) = Some(control);
    }

    /// Rebuild the server runtime from the stored settings and swap it in. On error the
    /// runtime in force is kept. Running modules are not restarted.
    pub fn reload_server(&self) -> Result<(), StartError> {
        let next = build_server(&self.settings)?;
        let level = next.cfg.log_level.clone();
        *self.server.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
        let control = self
            .log_control
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(control) = control
            && let Err(e) = control(&level)
        {
            warn!(error = %e, "could not apply the log level");
        }
        info!("server settings reloaded");
        Ok(())
    }

    pub fn sync_opt(&self) -> Option<Arc<SyncRuntime>> {
        self.sync_slot.get()
    }

    /// The running FlickSync, or `MODULE_DISABLED` (503).
    pub fn sync(&self) -> crate::errors::Result<Arc<SyncRuntime>> {
        self.sync_opt().ok_or_else(|| {
            Error::new(
                ErrorCode::ModuleDisabled,
                "FlickSync is not running on this server",
            )
        })
    }

    pub fn dd(&self) -> Option<Arc<DdState>> {
        self.dd_slot.get()
    }

    fn gate(&self, id: ModuleId) -> MutexGuard<'_, ()> {
        match id {
            ModuleId::FlickSync => self.sync_slot.gate(),
            ModuleId::FlickDd => self.dd_slot.gate(),
        }
    }

    fn build_sync(&self) -> Result<SyncRuntime, String> {
        let cfg = self.settings.config().map_err(|e| e.to_string())?;
        let manager = Arc::new(RoomManager::new(
            cfg.manager.clone(),
            self.clock.clone(),
            self.metrics.clone(),
        ));
        Ok(SyncRuntime {
            manager,
            conn_limit: Arc::new(Semaphore::new(cfg.ws.max_connections)),
            ws: cfg.ws,
        })
    }

    fn build_dd(&self) -> Result<Arc<DdState>, String> {
        let cfg = self.settings.config().map_err(|e| e.to_string())?;
        if !cfg.dd.enabled {
            return Err("FlickDD is not enabled".to_owned());
        }
        Ok(DdState::new(cfg.dd))
    }

    /// Start `id` with the settings in force. The caller holds the module's gate. A module that
    /// is already running is left alone; a failure leaves it `Failed`, never panics.
    fn start_locked(&self, id: ModuleId) {
        let fingerprint = self.settings.fingerprint(id.scope());
        match id {
            ModuleId::FlickSync => {
                if self.sync_slot.get().is_some() {
                    return;
                }
                match self.build_sync() {
                    Ok(rt) => {
                        self.sync_slot
                            .replace(SlotState::Running(Running::new(Arc::new(rt), fingerprint)));
                        info!("FlickSync started");
                    }
                    Err(message) => {
                        warn!(error = %message, "FlickSync could not start");
                        self.sync_slot.replace(SlotState::Failed(message));
                    }
                }
            }
            ModuleId::FlickDd => {
                if self.dd_slot.get().is_some() {
                    return;
                }
                match self.build_dd() {
                    Ok(rt) => {
                        self.dd_slot
                            .replace(SlotState::Running(Running::new(rt, fingerprint)));
                        info!("FlickDD started");
                    }
                    Err(message) => {
                        warn!(error = %message, "FlickDD could not start");
                        self.dd_slot.replace(SlotState::Failed(message));
                    }
                }
            }
        }
    }

    pub fn module_status(&self, id: ModuleId) -> crate::modules::ModuleStatus {
        let enabled = self.settings.is_enabled(id.scope());
        let fingerprint = self.settings.fingerprint(id.scope());
        match id {
            ModuleId::FlickSync => self.sync_slot.status(id, enabled, fingerprint),
            ModuleId::FlickDd => self.dd_slot.status(id, enabled, fingerprint),
        }
    }

    /// Stop `id` and free its runtime. The caller holds the module's gate.
    fn stop_locked(&self, id: ModuleId) {
        match id {
            ModuleId::FlickSync => {
                if let SlotState::Running(r) = self.sync_slot.replace(SlotState::Stopped) {
                    r.rt.manager.shutdown();
                    info!("FlickSync stopped");
                }
            }
            ModuleId::FlickDd => {
                if let SlotState::Running(r) = self.dd_slot.replace(SlotState::Stopped) {
                    let cut = r.rt.shutdown();
                    info!(streams = cut, "FlickDD stopped");
                }
            }
        }
    }

    /// Persist "enabled" and start the module. The gate is taken first so that the persisted
    /// switch and the runtime cannot be reordered by a concurrent call.
    pub fn start_module(
        &self,
        id: ModuleId,
    ) -> Result<crate::modules::ModuleStatus, crate::settings::SettingsError> {
        let _gate = self.gate(id);
        self.settings.set_enabled(id.scope(), true)?;
        self.start_locked(id);
        Ok(self.module_status(id))
    }

    /// Persist "disabled" and stop the module.
    pub fn stop_module(
        &self,
        id: ModuleId,
    ) -> Result<crate::modules::ModuleStatus, crate::settings::SettingsError> {
        let _gate = self.gate(id);
        self.settings.set_enabled(id.scope(), false)?;
        self.stop_locked(id);
        Ok(self.module_status(id))
    }

    /// Stop then start with the settings now stored. A disabled module stays stopped.
    pub fn reload_module(&self, id: ModuleId) -> crate::modules::ModuleStatus {
        let _gate = self.gate(id);
        self.stop_locked(id);
        if self.settings.is_enabled(id.scope()) {
            self.start_locked(id);
        }
        self.module_status(id)
    }

    /// Process shutdown: close everything, keep the persisted switches as they are.
    pub fn stop_all(&self) {
        for id in ModuleId::ALL {
            let _gate = self.gate(id);
            self.stop_locked(id);
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    crate::api::router(state)
}

/// Periodic maintenance of whatever is running (reconnection grace, expiry, sync heartbeat).
pub fn spawn_sweeper(state: &AppState) -> JoinHandle<()> {
    let state = state.clone();
    let period = Duration::from_millis(state.boot.sweep_interval_ms.max(10));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Some(sync) = state.sync_opt() {
                sync.manager.sweep();
            }
            if let Some(dd) = state.dd() {
                dd.sweep();
            }
        }
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Bind and serve until SIGINT/SIGTERM.
pub async fn serve(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", state.boot.host, state.boot.port).parse()?;
    let grace = Duration::from_secs(state.boot.shutdown_grace_secs);
    let sweeper = spawn_sweeper(&state);
    let app = build_router(state.clone());
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        version = env!("CARGO_PKG_VERSION"),
        auth_keys = state.server().cfg.auth.keys.len(),
        "flicksync listening"
    );

    let stopping = state.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        info!("shutdown signal received");
        // Close rooms and sockets first, otherwise open WebSockets would hold shutdown up;
        // same for download streams, a throttled file response can last hours.
        stopping.stop_all();
    });

    tokio::select! {
        res = server => {
            if let Err(e) = res {
                error!(error = %e, "server error");
                return Err(e.into());
            }
        }
        _ = async {
            shutdown_signal().await;
            tokio::time::sleep(grace).await;
        } => {
            info!("grace period elapsed, exiting");
        }
    }
    sweeper.abort();
    Ok(())
}

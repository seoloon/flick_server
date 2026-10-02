//! Application wiring: shared state, background tasks and the server loop.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::auth::{AuthConfigError, Authenticator};
use crate::config::Config;
use crate::metrics::Metrics;
use crate::room::RoomManager;
use crate::sync::{Clock, SystemClock};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub manager: Arc<RoomManager>,
    pub auth: Arc<Authenticator>,
    pub metrics: Arc<Metrics>,
    pub conn_limit: Arc<Semaphore>,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(cfg: Config) -> Result<Self, AuthConfigError> {
        Self::with_clock(cfg, Arc::new(SystemClock::new()))
    }

    pub fn with_clock(cfg: Config, clock: Arc<dyn Clock>) -> Result<Self, AuthConfigError> {
        let metrics = Arc::new(Metrics::default());
        let auth = Arc::new(Authenticator::new(&cfg.auth)?);
        let manager = Arc::new(RoomManager::new(
            cfg.manager.clone(),
            clock,
            metrics.clone(),
        ));
        let conn_limit = Arc::new(Semaphore::new(cfg.ws.max_connections));
        Ok(Self {
            cfg: Arc::new(cfg),
            manager,
            auth,
            metrics,
            conn_limit,
            started_at: Instant::now(),
        })
    }
}

pub fn build_router(state: AppState) -> Router {
    crate::api::router(state)
}

/// Periodic room maintenance (reconnection grace, expiry, sync heartbeat).
pub fn spawn_sweeper(state: &AppState) -> JoinHandle<()> {
    let manager = state.manager.clone();
    let period = Duration::from_millis(state.cfg.sweep_interval_ms.max(10));
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            manager.sweep();
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
pub async fn serve(cfg: Config) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let grace = Duration::from_secs(cfg.shutdown_grace_secs);
    let state = AppState::new(cfg)?;
    let sweeper = spawn_sweeper(&state);
    let app = build_router(state.clone());
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        version = env!("CARGO_PKG_VERSION"),
        auth_keys = state.cfg.auth.keys.len(),
        "flicksync listening"
    );

    let manager = state.manager.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown_signal().await;
        info!("shutdown signal received");
        // Close rooms and sockets first, otherwise open WebSockets would hold shutdown up.
        manager.shutdown();
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

//! FlickSync: a small real-time watch-together coordination service for Flick.
//!
//! Layering (inner to outer):
//! * [`sync`], [`protocol`], [`chat`], [`ratelimit`]: pure domain building blocks.
//! * [`room`]: the room state machine (pure) and the manager (locks + delivery).
//! * [`auth`], [`config`], [`metrics`]: cross-cutting services.
//! * [`api`], [`websocket`], [`app`]: the Axum/WebSocket adapter.

pub mod api;
pub mod app;
pub mod auth;
pub mod chat;
pub mod config;
pub mod errors;
pub mod metrics;
pub mod protocol;
pub mod ratelimit;
pub mod room;
pub mod sync;
pub mod websocket;

//! FlickSync: a small real-time watch-together coordination service for Flick.
//!
//! Layering (inner to outer):
//! * [`sync`], [`protocol`], [`chat`], [`ratelimit`]: pure domain building blocks.
//! * [`room`]: the room state machine (pure) and the manager (locks + delivery).
//! * [`auth`], [`config`], [`metrics`]: cross-cutting services.
//! * [`api`], [`websocket`], [`app`]: the Axum/WebSocket adapter.

pub mod admin_token;
pub mod api;
pub mod app;
pub mod auth;
pub mod chat;
pub mod config;
pub mod dd;
pub mod errors;
pub mod invite;
pub mod logs;
pub mod metrics;
pub mod modules;
pub mod protocol;
pub mod ratelimit;
pub mod room;
pub mod settings;
pub mod sync;
pub mod websocket;

//! One task per WebSocket connection.
//!
//! The task only moves bytes: parsing is delegated to the protocol module and
//! every decision to the room manager. It enforces transport-level concerns:
//! frame types, idle detection (ping/pong), write timeouts and flood handling.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::{Instant, MissedTickBehavior, interval, timeout};
use tracing::{debug, info};

use crate::app::AppState;
use crate::auth::Identity;
use crate::errors::{Error, ErrorCode};
use crate::metrics::Metrics;
use crate::protocol::{ClientMessage, ServerMessage};
use crate::room::manager::{Attachment, CloseReason, Outbound};

const CLOSE_HANDSHAKE_WAIT: Duration = Duration::from_secs(1);

/// Keeps the open-connections gauge honest on every exit path.
struct ConnGauge(Arc<Metrics>);

impl ConnGauge {
    fn new(m: Arc<Metrics>) -> Self {
        m.ws_connections.inc();
        Self(m)
    }
}

impl Drop for ConnGauge {
    fn drop(&mut self) {
        self.0.ws_connections.dec();
    }
}

fn close_frame(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

fn close_for(reason: CloseReason) -> Message {
    match reason {
        CloseReason::Replaced => close_frame(4001, "replaced by a newer connection"),
        CloseReason::Removed => close_frame(4002, "left the room"),
        CloseReason::RoomClosed => close_frame(4003, "room closed"),
        CloseReason::SlowConsumer => close_frame(1013, "too slow"),
        CloseReason::Shutdown => close_frame(1001, "server shutting down"),
    }
}

async fn send(socket: &mut WebSocket, msg: Message, limit: Duration) -> bool {
    matches!(timeout(limit, socket.send(msg)).await, Ok(Ok(())))
}

async fn send_error(socket: &mut WebSocket, e: &Error, limit: Duration) -> bool {
    let text = ServerMessage::from_error(e).to_json();
    send(socket, Message::Text(text.into()), limit).await
}

pub async fn run(
    mut socket: WebSocket,
    state: AppState,
    room_id: String,
    identity: Identity,
    _permit: OwnedSemaphorePermit,
) {
    let cfg = &state.cfg.ws;
    let send_limit = Duration::from_secs(cfg.send_timeout_secs);
    let idle_limit = Duration::from_secs(cfg.idle_timeout_secs);
    let pid = identity.user_id.clone();

    let Attachment { conn_id, mut rx } = match state.manager.attach(&room_id, &identity) {
        Ok(a) => a,
        Err(e) => {
            debug!(room_id = %room_id, participant_id = %pid, code = ?e.code, "websocket attach refused");
            send_error(&mut socket, &e, send_limit).await;
            send(&mut socket, close_frame(1008, "attach refused"), send_limit).await;
            return;
        }
    };
    let _gauge = ConnGauge::new(state.metrics.clone());
    info!(room_id = %room_id, participant_id = %pid, server_id = %identity.server_id, "websocket connected");

    let mut ping = interval(Duration::from_secs(cfg.ping_interval_secs));
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_seen = Instant::now();
    let mut strikes = 0u32;

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let msg = match incoming {
                    None => break,
                    Some(Err(e)) => {
                        // Includes frames above the configured size limit.
                        debug!(room_id = %room_id, participant_id = %pid, error = %e, "websocket receive error");
                        break;
                    }
                    Some(Ok(m)) => m,
                };
                last_seen = Instant::now();
                match msg {
                    Message::Text(text) => {
                        state.metrics.messages_in_total.inc();
                        let result = if text.len() > cfg.max_message_bytes {
                            Err(Error::new(ErrorCode::MessageTooLarge, "message too large"))
                        } else {
                            match ClientMessage::parse(text.as_str()) {
                                Ok(m) => state.manager.handle_message(&room_id, &pid, conn_id, m),
                                Err(e) => {
                                    state.metrics.malformed_messages_total.inc();
                                    debug!(room_id = %room_id, participant_id = %pid, code = ?e.code, "malformed message");
                                    Err(e)
                                }
                            }
                        };
                        match result {
                            Ok(()) => strikes = 0,
                            Err(e) => {
                                if e.code == ErrorCode::RateLimited {
                                    strikes += 1;
                                    if strikes >= cfg.rate_limit_strikes {
                                        info!(room_id = %room_id, participant_id = %pid, "closing connection: sustained flooding");
                                        send(&mut socket, close_frame(1008, "rate limit exceeded"), send_limit).await;
                                        break;
                                    }
                                }
                                if e.code == ErrorCode::SessionReplaced {
                                    send_error(&mut socket, &e, send_limit).await;
                                    break;
                                }
                                if !send_error(&mut socket, &e, send_limit).await {
                                    break;
                                }
                            }
                        }
                    }
                    Message::Binary(_) => {
                        state.metrics.malformed_messages_total.inc();
                        let e = Error::new(ErrorCode::InvalidMessage, "binary frames are not supported");
                        if !send_error(&mut socket, &e, send_limit).await {
                            break;
                        }
                    }
                    // Pings are answered by the library; any frame counts as liveness.
                    Message::Ping(_) | Message::Pong(_) => {}
                    Message::Close(_) => break,
                }
            }
            out = rx.recv() => {
                match out {
                    Some(Outbound::Text(t)) => {
                        if !send(&mut socket, Message::Text(t.as_ref().into()), send_limit).await {
                            break;
                        }
                    }
                    Some(Outbound::Close(reason)) => {
                        send(&mut socket, close_for(reason), send_limit).await;
                        break;
                    }
                    // The manager dropped us (e.g. slow consumer).
                    None => {
                        send(&mut socket, close_for(CloseReason::SlowConsumer), send_limit).await;
                        break;
                    }
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > idle_limit {
                    info!(room_id = %room_id, participant_id = %pid, "closing idle websocket");
                    break;
                }
                if !send(&mut socket, Message::Ping(Vec::new().into()), send_limit).await {
                    break;
                }
            }
        }
    }

    state.manager.detach(&room_id, &pid, conn_id);
    // Let the closing handshake finish: dropping the socket while the peer still has data
    // in flight can make the OS reset the connection and lose our final frames.
    let _ = timeout(CLOSE_HANDSHAKE_WAIT, async {
        while let Some(Ok(m)) = socket.recv().await {
            if matches!(m, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
    info!(room_id = %room_id, participant_id = %pid, "websocket disconnected");
}

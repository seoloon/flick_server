//! REST endpoints for room membership. Playback and chat are WebSocket-only.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::auth::Authed;
use super::error::ApiError;
use crate::app::AppState;
use crate::errors::{Error, ErrorCode};
use crate::protocol::{ControlMode, RoomView};
use crate::room::manager::{CreateOptions, share_code};

#[derive(Debug, Default, Deserialize)]
struct CreateRoomRequest {
    #[serde(default)]
    control_mode: Option<ControlMode>,
    #[serde(default)]
    chat_enabled: Option<bool>,
}

/// What a client needs to connect to a room.
#[derive(Debug, Serialize)]
pub struct RoomAccess {
    pub room_id: String,
    /// Grouped form of the id, convenient to read aloud / paste (accepted wherever an id is).
    pub share_code: String,
    pub participant_id: String,
    pub host_id: String,
    /// Path of the WebSocket endpoint, relative to the API base URL: the invitation address, with
    /// its path prefix when the service sits behind a proxy under one.
    pub ws_path: String,
    pub room: RoomView,
}

fn access(room: RoomView, participant_id: String) -> RoomAccess {
    RoomAccess {
        share_code: share_code(&room.room_id),
        ws_path: format!("/api/v1/rooms/{}/ws", room.room_id),
        host_id: room.host_id.clone(),
        room_id: room.room_id.clone(),
        participant_id,
        room,
    }
}

/// `POST /api/v1/rooms`: the caller becomes the host. Body is optional.
pub async fn create_room(
    State(state): State<AppState>,
    Authed(identity): Authed,
    body: Bytes,
) -> Result<(StatusCode, Json<RoomAccess>), ApiError> {
    let req: CreateRoomRequest = if body.iter().all(u8::is_ascii_whitespace) {
        CreateRoomRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| {
            Error::new(
                ErrorCode::InvalidPayload,
                "request body must be a JSON object",
            )
        })?
    };
    let created = state.manager.create_room(
        &identity,
        CreateOptions {
            control_mode: req.control_mode,
            chat_enabled: req.chat_enabled,
        },
    )?;
    Ok((
        StatusCode::CREATED,
        Json(access(created.room, created.participant_id)),
    ))
}

/// `GET /api/v1/rooms/{room_id}`: members only.
pub async fn get_room(
    State(state): State<AppState>,
    Authed(identity): Authed,
    Path(room_id): Path<String>,
) -> Result<Json<RoomView>, ApiError> {
    Ok(Json(state.manager.get_room(&room_id, &identity)?))
}

/// `POST /api/v1/rooms/{room_id}/join`: reserve a seat; open the WebSocket afterwards.
pub async fn join_room(
    State(state): State<AppState>,
    Authed(identity): Authed,
    Path(room_id): Path<String>,
) -> Result<Json<RoomAccess>, ApiError> {
    let room = state.manager.join(&room_id, &identity)?;
    Ok(Json(access(room, identity.user_id)))
}

/// `POST /api/v1/rooms/{room_id}/leave`
pub async fn leave_room(
    State(state): State<AppState>,
    Authed(identity): Authed,
    Path(room_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.manager.leave(&room_id, &identity)?;
    Ok(StatusCode::NO_CONTENT)
}

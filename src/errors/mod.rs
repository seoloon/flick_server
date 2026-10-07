//! Typed errors shared by the domain, the protocol and the HTTP/WebSocket adapters.
//!
//! Nothing in here knows about Axum: the HTTP mapping lives in `api::error`.

use serde::{Deserialize, Serialize};

/// Stable, machine-readable error codes. These are part of the wire protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    Unauthenticated,
    Forbidden,
    NotHost,
    NotMember,
    ControlDenied,
    RoomNotFound,
    RoomFull,
    RoomClosed,
    TooManyRooms,
    TooManyConnections,
    InvalidMessage,
    UnknownType,
    UnsupportedVersion,
    InvalidPayload,
    InvalidPosition,
    InvalidRate,
    InvalidSequence,
    InvalidMedia,
    NoMedia,
    MessageTooLarge,
    RateLimited,
    ChatDisabled,
    SessionReplaced,
    DownloadNotFound,
    TooManyDownloads,
    SourceChanged,
    QuotaExceeded,
    RangeNotSatisfiable,
    BackendUnavailable,
    ModuleDisabled,
    Internal,
}

impl ErrorCode {
    /// HTTP status used when the error is returned by the REST API.
    pub fn http_status(self) -> u16 {
        use ErrorCode::*;
        match self {
            Unauthenticated => 401,
            Forbidden | NotHost | NotMember | ControlDenied | ChatDisabled => 403,
            RoomNotFound | DownloadNotFound => 404,
            RoomFull | SessionReplaced | SourceChanged => 409,
            RoomClosed => 410,
            MessageTooLarge => 413,
            RateLimited | TooManyRooms | TooManyDownloads | QuotaExceeded => 429,
            RangeNotSatisfiable => 416,
            BackendUnavailable => 502,
            TooManyConnections | ModuleDisabled => 503,
            Internal => 500,
            InvalidMessage | UnknownType | UnsupportedVersion | InvalidPayload
            | InvalidPosition | InvalidRate | InvalidSequence | InvalidMedia | NoMedia => 400,
        }
    }
}

/// An error that is safe to show to a client (no internals, no stack traces).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_disabled_is_a_503_with_a_stable_wire_name() {
        assert_eq!(ErrorCode::ModuleDisabled.http_status(), 503);
        assert_eq!(
            serde_json::to_string(&ErrorCode::ModuleDisabled).unwrap(),
            "\"MODULE_DISABLED\""
        );
    }
}

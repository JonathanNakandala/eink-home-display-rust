//! What the admin API sends and receives, and how it fails.
//!
//! These are the wire types, shared by the server and the command line that talks to it, and they are what
//! the API description (`config/admin-openapi.json`) is made from. An answer that is an error is always
//! `{ "error": { "code": ..., "message": ... } }`: the code is for a program to act on and does not change, the
//! message is for a person and may.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoResponses, ToSchema};

/// The longest a pairing window can be opened for. Long enough for a display that wakes rarely; short enough
/// that the window is not left open and forgotten.
pub const MAX_WINDOW_MINUTES: u32 = 240;

/// Whether displays can ask to join right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WindowState {
    /// Whether the window is open. While it is, a display that has not joined can ask to, and a display that lost
    /// its key can ask to take its name back. Closed, those requests are turned away; a display already waiting
    /// can still be approved.
    pub open: bool,
    /// When the window closes by itself. Only present while it is open.
    #[schema(examples("2026-10-08T12:15:00Z"))]
    pub closes_at: Option<DateTime<Utc>>,
}

/// How long to open the pairing window for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OpenWindow {
    /// Minutes from now, from 1 to 240. Opening an open window again sets the time afresh.
    #[schema(minimum = 1, maximum = 240, examples(15))]
    pub minutes: u32,
}

/// Why a request failed, for a program to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request could not be understood: no body, a body that is not JSON, or one of the wrong shape.
    InvalidRequest,
    /// `minutes` is not a whole number from 1 to 240.
    InvalidMinutes,
    /// There is nothing at that path.
    NotFound,
    /// That path exists but not for that method.
    MethodNotAllowed,
    /// The server failed. The log has the detail.
    Internal,
}

impl ErrorCode {
    fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest | Self::InvalidMinutes => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The answer to a request that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub code: ErrorCode,
    /// For a person to read. Its wording may change; act on `code`.
    #[schema(examples("minutes must be a whole number from 1 to 240"))]
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            error: ErrorBody {
                code,
                message: message.into(),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.error.code.status(), Json(self)).into_response()
    }
}

/// The answer of an endpoint that takes input, when the input could not be used.
#[derive(IntoResponses)]
#[allow(dead_code)]
pub enum BadRequest {
    /// The request could not be used. `code` says why.
    #[response(status = 400)]
    BadRequest(#[to_schema] ApiError),
}

/// The answer of any endpoint when the server itself fails.
#[derive(IntoResponses)]
#[allow(dead_code)]
pub enum ServerFailure {
    /// The server failed. The log has the detail.
    #[response(status = 500)]
    Internal(#[to_schema] ApiError),
}

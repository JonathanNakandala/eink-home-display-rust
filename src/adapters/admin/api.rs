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
use utoipa::{IntoParams, IntoResponses, ToSchema};

use crate::domain::models::pairing::{Pairing, PairingState, renewal_overdue};

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

/// Where a display stands. A name appears here as soon as it asks to join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisplayState {
    /// Asked to join and is waiting for the owner to approve it, with the code on its own panel.
    Waiting,
    /// Approved. It becomes a member the next time it asks, which is within a few minutes.
    Approved,
    /// Joined: it has a certificate and is served.
    Member,
    /// Turned down by the owner. It stays turned down until it is forgotten.
    Rejected,
    /// Was a member and no longer is.
    Revoked,
}

/// A display the server knows, and where it stands. Nothing here is secret: it never carries a pairing code, a key
/// or a certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DisplayEntry {
    /// The name the display gives itself.
    #[schema(
        examples("reterminal-e1003-a1b2c3"),
        max_length = 32,
        pattern = "^[A-Za-z0-9._-]+$"
    )]
    pub name: String,
    pub state: DisplayState,
    /// When the request that is waiting was made: the display's own request to join, or (for a member) another
    /// key's request to take its name. A request lapses a day after this if nobody answers it.
    pub waiting_since: Option<DateTime<Utc>>,
    /// For a member, when the latest certificate it was given ends.
    pub certificate_not_after: Option<DateTime<Utc>>,
    /// A member whose certificate has run out. Nothing to fix: it gets a new one by itself, with no one at the
    /// server, the next time it is switched on.
    pub certificate_expired: bool,
    /// A member whose certificate should have been renewed by now and has not been (about 63 days after it was issued,
    /// for a 90-day certificate): it has stopped renewing, though the certificate has not run out. Look at the display;
    /// once the certificate ends this is false and `certificate_expired` says so.
    pub renewal_overdue: bool,
    /// A member that has been given a certificate for a new key and has not used it yet.
    pub changing_keys: bool,
    /// A different key is waiting for the owner to approve it taking this member's name, as when a display was
    /// reflashed and lost its key. The member carries on meanwhile.
    pub replacement_waiting: bool,
}

impl DisplayEntry {
    /// `lifetime` is how long the certificates this server issues last.
    pub fn of(pairing: &Pairing, now: DateTime<Utc>, lifetime: chrono::Duration) -> Self {
        let (state, ends) = match &pairing.state {
            PairingState::Pending => (DisplayState::Waiting, None),
            PairingState::Approved => (DisplayState::Approved, None),
            PairingState::Enrolled { not_after, .. } => (DisplayState::Member, Some(*not_after)),
            PairingState::Rejected => (DisplayState::Rejected, None),
            PairingState::Revoked => (DisplayState::Revoked, None),
        };
        let waiting_since = match (&pairing.state, &pairing.replacement) {
            (PairingState::Pending, _) => Some(pairing.requested_at),
            (_, Some(replacement)) => Some(replacement.requested_at),
            _ => None,
        };
        Self {
            name: pairing.device.to_string(),
            state,
            waiting_since,
            certificate_not_after: ends,
            certificate_expired: ends.is_some_and(|end| end <= now),
            renewal_overdue: ends.is_some_and(|end| renewal_overdue(end - now, lifetime)),
            changing_keys: pairing.rollover.is_some(),
            replacement_waiting: pairing.replacement.is_some(),
        }
    }
}

/// Every display the server knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DisplayList {
    /// By name.
    pub displays: Vec<DisplayEntry>,
}

/// The name of a display, in the path.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct DisplayPath {
    /// The display's name, as `displays` lists it.
    #[param(
        max_length = 32,
        pattern = "^[A-Za-z0-9._-]+$",
        example = "reterminal-e1003-a1b2c3"
    )]
    pub name: String,
}

/// What the owner read off the display's own panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApproveRequest {
    /// The pairing code on the display's own panel: twelve letters and digits, in any case, with or without the
    /// dashes. `I` and `L` are read as `1` and `O` as `0`. Type it from the panel, never from anywhere else: the
    /// server does not show it, because a code copied from the server would say nothing about the display.
    #[schema(examples("B0AJ-QTW6-Y8SA"))]
    pub code: String,
}

/// Why a request failed, for a program to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request could not be understood: no body, a body that is not JSON, or one of the wrong shape.
    InvalidRequest,
    /// `minutes` is not a whole number from 1 to 240.
    InvalidMinutes,
    /// The display's name in the path is not a valid name (letters, digits, `-`, `_` and `.`, at most 32, and not
    /// only dots).
    InvalidName,
    /// The code is not twelve letters and digits. Nothing was tried against the display.
    InvalidCode,
    /// The code does not match what the display would show. A typing mistake, or, if it is right on the
    /// display's panel, something between the display and the server.
    WrongCode,
    /// Too many wrong codes were typed for the display in a row, so none is looked at for a while (the message says how
    /// long). Look at the code on the display's panel and at the one the server shows; if they differ, the display may be
    /// talking to something else.
    TooManyAttempts,
    /// No display of that name has asked to join, or its request lapsed.
    UnknownDisplay,
    /// The display is not waiting for approval, so there is nothing to approve or turn down.
    NotWaiting,
    /// The display is not a member, so there is nothing to revoke.
    NotAMember,
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
            Self::InvalidRequest | Self::InvalidMinutes | Self::InvalidName | Self::InvalidCode => {
                StatusCode::BAD_REQUEST
            }
            Self::WrongCode => StatusCode::FORBIDDEN,
            Self::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
            Self::NotFound | Self::UnknownDisplay => StatusCode::NOT_FOUND,
            Self::NotWaiting | Self::NotAMember => StatusCode::CONFLICT,
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

impl ApiError {
    /// The server failed: the detail goes to the log, and the caller is told only that it did.
    pub fn internal(what: &str, error: impl std::fmt::Display) -> Self {
        log::error!("Admin API: failed to {what}: {error}");
        Self::new(
            ErrorCode::Internal,
            format!("The server failed to {what}; the log has the detail"),
        )
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

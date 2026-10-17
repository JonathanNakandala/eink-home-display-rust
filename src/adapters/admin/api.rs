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
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, IntoResponses, ToSchema};

use crate::application::credentials::{CredentialStatus, Origin};
use crate::application::status::ProfileStatus;
use crate::domain::models::layout::LayoutName;
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
    /// What the display said it is when it last asked: its model, panel, image formats and firmware. Self-asserted: for
    /// the owner to see before approving, never a reason to approve.
    pub profile: Option<ProfileStatus>,
    /// What the key that is waiting to take this member's name says it is, if there is one and it said.
    pub replacement_profile: Option<ProfileStatus>,
    /// The layout the owner chose for this display, which it is sent when it doesn't ask for one. Left out, it gets the
    /// default layout.
    #[schema(examples("departures"))]
    pub layout: Option<String>,
}

impl DisplayEntry {
    /// This display with the layout chosen for it, if one was (which is kept apart from the pairing).
    pub fn with_layout(mut self, layout: Option<&LayoutName>) -> Self {
        self.layout = layout.map(ToString::to_string);
        self
    }

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
            profile: pairing.profile.as_ref().map(ProfileStatus::from),
            replacement_profile: pairing
                .replacement
                .as_ref()
                .and_then(|replacement| replacement.profile.as_ref())
                .map(ProfileStatus::from),
            layout: None,
        }
    }
}

/// The layouts a display can be given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LayoutList {
    /// The default first.
    pub layouts: Vec<LayoutEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LayoutEntry {
    #[schema(examples("departures"))]
    pub name: String,
    /// What the layout shows.
    #[schema(examples("Departures alone, in larger type"))]
    pub description: String,
    /// The layout a display with none chosen is given if the schedule doesn't say otherwise.
    pub default: bool,
    /// What a display with none chosen is sent now: the default, unless the layout schedule says another for this
    /// time.
    pub current: bool,
}

/// What a viewer is asking of the server to be: the label the owner will know it by, and what it may see.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
pub struct IssueViewer {
    /// What to call it, from 1 to 40 characters: "kitchen tablet".
    #[schema(examples("kitchen tablet"))]
    pub label: String,
    /// The layouts it may look at, by name. Left out or empty, all of them.
    #[serde(default)]
    pub layouts: Vec<String>,
    /// How long its token lasts once the link is used, from 1 to 3650 days. Left out, 90.
    #[serde(default)]
    pub lifetime_days: Option<u32>,
    /// How long the link works, from 1 to 1440 minutes. Left out, 15.
    #[serde(default)]
    pub code_minutes: Option<u32>,
    /// Give it short-lived access tokens (an hour, unless `[server.web] access_token_minutes` says) and a rotating refresh
    /// token to renew them with, in place of one token that lasts `lifetime_days`. A token that is lost is then good for an
    /// hour and not a quarter of a year, and a refresh token that is copied is noticed, which ends the session. `lifetime_days`
    /// is then how long the session lasts.
    #[serde(default)]
    pub refresh: bool,
}

/// Where a viewer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewerStatus {
    /// Its link has not been used, and still can be.
    Waiting,
    /// Its link was not used in time. Revoke it and make another.
    LinkExpired,
    /// Signed in, and the token works.
    Active,
    /// The token ran out. Make another viewer.
    Expired,
    Revoked,
}

impl From<crate::domain::models::viewer::ViewerStanding> for ViewerStatus {
    fn from(standing: crate::domain::models::viewer::ViewerStanding) -> Self {
        use crate::domain::models::viewer::ViewerStanding as S;
        match standing {
            S::Waiting => Self::Waiting,
            S::LinkExpired => Self::LinkExpired,
            S::Active => Self::Active,
            S::Expired => Self::Expired,
            S::Revoked => Self::Revoked,
        }
    }
}

/// A viewer, as the owner sees it. Nothing here can be used to sign in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ViewerEntry {
    /// What to give `viewer revoke` and `viewer forget`.
    #[schema(examples("k3x6qa2m"), pattern = "^[a-z2-7]{8}$")]
    pub id: String,
    pub label: String,
    pub status: ViewerStatus,
    /// What it may look at: `layout:*`, or `layout:NAME` for each layout.
    #[schema(examples("layout:*"))]
    pub scope: String,
    /// Whether it has short-lived access tokens and a refresh token, and not one token that lasts.
    pub refreshing: bool,
    pub created_at: DateTime<Utc>,
    /// For one waiting, when its link stops working.
    pub link_expires_at: Option<DateTime<Utc>>,
    /// Once signed in, when its access ends: for a plain viewer, when the token runs out, and for a refreshing one, when the
    /// session does (its access tokens run out and are renewed all the time).
    pub expires_at: Option<DateTime<Utc>>,
    /// When it was last used, to within ten minutes or so.
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ViewerEntry {
    pub fn of(viewer: &crate::domain::models::viewer::Viewer, now: DateTime<Utc>) -> Self {
        use crate::domain::models::viewer::ViewerState;
        let (link_expires_at, expires_at, last_used_at) = match &viewer.state {
            ViewerState::Pending {
                code_expires_at, ..
            } => (Some(*code_expires_at), None, None),
            ViewerState::Active {
                expires_at,
                last_used_at,
                refresh,
                ..
            } => (
                None,
                Some(refresh.as_ref().map_or(*expires_at, |r| r.session_ends_at)),
                *last_used_at,
            ),
            ViewerState::Revoked { .. } => (None, None, None),
        };
        Self {
            id: viewer.id.to_string(),
            label: viewer.label.to_string(),
            status: viewer.standing(now).into(),
            scope: viewer.scope.to_scope_string(),
            refreshing: viewer.refreshing,
            created_at: viewer.created_at,
            link_expires_at,
            expires_at,
            last_used_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ViewerList {
    /// Oldest first.
    pub viewers: Vec<ViewerEntry>,
}

/// A viewer made, with the link to sign it in. The only answer of this API that holds a secret, and it is shown only now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ViewerIssued {
    pub viewer: ViewerEntry,
    /// The one-time code, which a browser exchanges for a token. It is in `link`; it is here for a program that builds its
    /// own.
    #[schema(examples("hdc1_…"))]
    pub code: String,
    /// What to open in the browser to sign it in, once, before `link_expires_at`: the page, with the code after a `#`, which
    /// a browser keeps to itself and never sends to the server.
    #[schema(examples("https://display.local:8443/view#code=hdc1_…"))]
    pub link: String,
    pub link_expires_at: DateTime<Utc>,
    /// Things the owner should know about what this viewer will see, such as a layout the page is set not to show. The viewer is
    /// made all the same.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The id of a viewer, in the path.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ViewerPath {
    /// The viewer's id, as `viewers` lists it.
    #[param(pattern = "^[a-z2-7]{8}$", example = "k3x6qa2m")]
    pub id: String,
}

/// The layout to give a display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChooseLayout {
    /// One of the names `GET /v1/layouts` lists.
    #[schema(examples("departures"))]
    pub layout: String,
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
    #[schema(examples("JGWP-14YW-3BT0"))]
    pub code: String,
}

/// Where the key in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// The server's configuration file.
    Config,
    /// Set with this API, and in the server's memory alone: gone when the server stops.
    Set,
    /// Set with this API and asked to be remembered, then read back from where the server keeps it when it started.
    Remembered,
}

/// One of the keys the server uses for other services, and where it stands. Never the key itself: nothing in this API
/// returns one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CredentialEntry {
    /// The key's name, as the path takes it.
    #[schema(examples("open-weather"))]
    pub name: String,
    /// What it is for.
    #[schema(examples("OpenWeather API key (weather)"))]
    pub description: String,
    /// Whether the service answers without a key, so that having none is a choice and not a gap.
    pub optional: bool,
    /// Whether there is a key now.
    pub set: bool,
    /// Where the key in use came from. Only present when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<KeySource>,
    /// When it was set with this API. Not present for a key from the configuration file, or from an earlier run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<DateTime<Utc>>,
    /// Whether a copy is kept on disk, so that the key outlasts a restart.
    pub remembered: bool,
}

impl CredentialEntry {
    pub fn of(status: &CredentialStatus) -> Self {
        Self {
            name: status.name.as_str().to_owned(),
            description: status.name.describe().to_owned(),
            optional: status.name.is_optional(),
            set: status.is_set(),
            source: status.origin.map(|origin| match origin {
                Origin::Config => KeySource::Config,
                Origin::Set => KeySource::Set,
                Origin::Remembered => KeySource::Remembered,
            }),
            since: status.since,
            remembered: status.remembered,
        }
    }
}

/// Every key the server can hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CredentialList {
    pub credentials: Vec<CredentialEntry>,
}

/// The name of a key, in the path.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct CredentialPath {
    /// The key's name, as `GET /v1/credentials` lists it.
    #[param(example = "open-weather")]
    pub name: String,
}

/// A key to use from now on. It is sent in the body, never in the path or a query, so it is in no log of requests.
#[derive(Deserialize, ToSchema)]
pub struct SetCredential {
    /// The key, exactly as the service gave it. Plain text with no spaces or line breaks; an OpenWeather key is 32
    /// characters. The answer never repeats it.
    #[schema(value_type = String, format = Password)]
    pub key: SecretString,
    /// Keep the key on disk too, in a file only the server's user can read, so that it is still there after a restart.
    /// Without this the key is in memory alone, and setting it also drops any copy kept before.
    #[serde(default)]
    pub remember: bool,
}

impl std::fmt::Debug for SetCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetCredential")
            .field("key", &"[REDACTED]")
            .field("remember", &self.remember)
            .finish()
    }
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
    /// No layout of that name exists (or the name could not be one). `GET /v1/layouts` lists them.
    UnknownLayout,
    /// The name in the path is not one of the keys the server holds. `GET /v1/credentials` lists them.
    UnknownCredential,
    /// The key cannot be one: the wrong length for its service, or with a space, a line break or another character that is
    /// not plain text. The message says which, and never repeats the key.
    InvalidCredential,
    /// The key was asked to be remembered but the server has nowhere to keep it (it keeps them with the pairing data, which
    /// it has only when it serves HTTPS). Set it without `remember`, or serve HTTPS.
    CannotRemember,
    /// What a new viewer was asked for is not allowed: the label, how long its token lasts, or how long its link works.
    InvalidViewer,
    /// The viewer's id in the path is not an id (eight lower-case letters and digits).
    InvalidViewerId,
    /// No viewer has that id.
    UnknownViewer,
    /// There are as many viewers as there may be; forget some that are no longer wanted.
    TooManyViewers,
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
            Self::InvalidRequest
            | Self::InvalidMinutes
            | Self::InvalidName
            | Self::InvalidCode
            | Self::InvalidViewer
            | Self::InvalidViewerId
            | Self::InvalidCredential => StatusCode::BAD_REQUEST,
            Self::WrongCode => StatusCode::FORBIDDEN,
            Self::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
            Self::NotFound
            | Self::UnknownDisplay
            | Self::UnknownLayout
            | Self::UnknownViewer
            | Self::UnknownCredential => StatusCode::NOT_FOUND,
            Self::NotWaiting | Self::NotAMember | Self::TooManyViewers | Self::CannotRemember => {
                StatusCode::CONFLICT
            }
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

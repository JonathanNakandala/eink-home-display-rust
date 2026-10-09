//! The admin API's endpoints.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::response::{IntoResponse, Response};
use tower_http::timeout::TimeoutLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::api::{
    ApiError, ApproveRequest, BadRequest, DisplayEntry, DisplayList, DisplayPath, DisplayState,
    ErrorBody, ErrorCode, MAX_WINDOW_MINUTES, OpenWindow, ServerFailure, WindowState,
};
use crate::application::enrollment::{ApproveError, Enrollment};
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::PairingCode;

/// More than any request here needs.
const MAX_BODY: usize = 4096;
/// A request to a local server that takes longer than this has gone wrong.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct Admin {
    pub(super) enrollment: Arc<Enrollment>,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "E-ink home display admin API",
        description = "How the owner of a running server looks at it and changes it, from the same machine. \
            Served over a local socket (a Unix domain socket in the PKI directory, mode 0600), not over the \
            network. No endpoint returns a pairing code, a key or a certificate."
    ),
    // The error body is only reached through `IntoResponses`, which does not collect the schemas it names, so they
    // are listed here or the references to them would point at nothing.
    components(schemas(ApiError, ErrorBody, ErrorCode, DisplayState)),
    tags(
        (name = "pairing", description = "Letting displays join."),
        (name = "displays", description = "The displays the server knows, and what the owner decides about them.")
    )
)]
struct ApiDoc;

fn routes() -> OpenApiRouter<Arc<Admin>> {
    let mut description = ApiDoc::openapi();
    description.info.license = None;
    OpenApiRouter::with_openapi(description)
        .routes(routes!(show_window, open_window, close_window))
        .routes(routes!(list_displays))
        .routes(routes!(show_display, forget_display))
        .routes(routes!(approve_display))
        .routes(routes!(reject_display))
        .routes(routes!(revoke_display))
}

/// The API description (OpenAPI 3.1), as committed in `config/admin-openapi.json` and served at `/openapi.json`.
pub fn openapi_json() -> String {
    let (_, api) = routes().split_for_parts();
    api.to_pretty_json()
        .expect("the API description serializes")
        + "\n"
}

pub fn router(enrollment: Arc<Enrollment>) -> Router {
    let (router, _) = routes().split_for_parts();
    let description: Arc<str> = openapi_json().into();
    router
        .route(
            "/openapi.json",
            axum::routing::get(move || {
                let description = description.clone();
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        description.to_string(),
                    )
                }
            }),
        )
        .fallback(|| async { ApiError::new(ErrorCode::NotFound, "There is nothing at that path") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                ErrorCode::MethodNotAllowed,
                "That path does not take that method",
            )
        })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .with_state(Arc::new(Admin { enrollment }))
}

fn window_state(admin: &Admin) -> WindowState {
    let closes_at = admin.enrollment.window_closes_at();
    WindowState {
        open: closes_at.is_some(),
        closes_at,
    }
}

/// Whether displays can ask to join now, and until when.
#[utoipa::path(
    get,
    path = "/v1/window",
    tag = "pairing",
    responses(
        (status = 200, description = "The state of the pairing window.", body = WindowState),
        ServerFailure
    )
)]
async fn show_window(State(admin): State<Arc<Admin>>) -> Json<WindowState> {
    Json(window_state(&admin))
}

/// Opens the pairing window, so a display that has not joined can ask to.
#[utoipa::path(
    put,
    path = "/v1/window",
    tag = "pairing",
    request_body(content = OpenWindow, description = "How long to open it for."),
    responses(
        (status = 200, description = "The window is open, and until when.", body = WindowState),
        BadRequest,
        ServerFailure
    )
)]
async fn open_window(
    State(admin): State<Arc<Admin>>,
    body: Result<Json<OpenWindow>, JsonRejection>,
) -> Result<Json<WindowState>, ApiError> {
    let Json(OpenWindow { minutes }) = body.map_err(|rejection| {
        ApiError::new(
            ErrorCode::InvalidRequest,
            format!(
                "The body must be JSON like {{\"minutes\": 15}} ({})",
                rejection.body_text()
            ),
        )
    })?;
    if !(1..=MAX_WINDOW_MINUTES).contains(&minutes) {
        return Err(ApiError::new(
            ErrorCode::InvalidMinutes,
            format!("minutes must be a whole number from 1 to {MAX_WINDOW_MINUTES}"),
        ));
    }
    let closes = admin
        .enrollment
        .open_window(chrono::Duration::minutes(minutes.into()));
    log::info!(
        "The pairing window is open for {minutes} minutes (until {})",
        closes.format("%H:%M:%S UTC")
    );
    Ok(Json(window_state(&admin)))
}

/// Closes the pairing window. A display already waiting can still be approved.
#[utoipa::path(
    delete,
    path = "/v1/window",
    tag = "pairing",
    responses(
        (status = 200, description = "The window is closed (it may have been already).", body = WindowState),
        ServerFailure
    )
)]
async fn close_window(State(admin): State<Arc<Admin>>) -> Response {
    admin.enrollment.close_window();
    log::info!("The pairing window is closed");
    Json(window_state(&admin)).into_response()
}

// --- displays ----------------------------------------------------------------------------------------

/// A name from the path, as a name or the reason it is not one.
fn device_id(name: &str) -> Result<DeviceId, ApiError> {
    DeviceId::parse(name).map_err(|e| ApiError::new(ErrorCode::InvalidName, e.to_string()))
}

fn unknown(device: &DeviceId) -> ApiError {
    ApiError::new(
        ErrorCode::UnknownDisplay,
        format!("No display called {device} has asked to join, or its request lapsed"),
    )
}

/// What the owner decided about `device` failed: why, in the API's terms. `not_a_member` is the answer for
/// revoking; for the rest, a display in the wrong state is one that is not waiting.
fn refused(error: ApproveError, revoking: bool) -> ApiError {
    match error {
        ApproveError::Unknown(device) => unknown(&device),
        ApproveError::NotPending { device, state } if revoking => ApiError::new(
            ErrorCode::NotAMember,
            format!("{device} is {state}, not a member, so there is nothing to revoke"),
        ),
        ApproveError::NotPending { device, state } => ApiError::new(
            ErrorCode::NotWaiting,
            format!("{device} is {state}, not waiting for approval"),
        ),
        ApproveError::WrongCode(device) => ApiError::new(
            ErrorCode::WrongCode,
            format!(
                "That is not the code {device} would show. Type what the display's own panel shows. If it is \
                 right on the panel and still not accepted, something may be between the display and this server"
            ),
        ),
        ApproveError::TooManyAttempts {
            device,
            retry_after,
        } => ApiError::new(
            ErrorCode::TooManyAttempts,
            format!(
                "Too many wrong codes for {device} in a row, so none is accepted for {} more minute(s). Compare \
                 the code on its panel with the one the server shows: if they differ, the display may be \
                 talking to something else",
                retry_after.num_minutes().max(1)
            ),
        ),
        ApproveError::Failed(e) => ApiError::internal("carry that out", e),
    }
}

/// `device` as it stands now.
async fn entry_of(admin: &Admin, device: &DeviceId) -> Result<DisplayEntry, ApiError> {
    match admin.enrollment.pairing(device).await {
        Ok(Some(pairing)) => Ok(DisplayEntry::of(
            &pairing,
            admin.enrollment.now(),
            admin.enrollment.certificate_lifetime(),
        )),
        Ok(None) => Err(unknown(device)),
        Err(e) => Err(ApiError::internal("look the display up", e)),
    }
}

/// Every display the server knows, by name.
#[utoipa::path(
    get,
    path = "/v1/displays",
    tag = "displays",
    responses(
        (status = 200, description = "The displays, with where each stands. Requests that lapsed are not listed.", body = DisplayList),
        ServerFailure
    )
)]
async fn list_displays(State(admin): State<Arc<Admin>>) -> Result<Json<DisplayList>, ApiError> {
    let pairings = admin
        .enrollment
        .pairings()
        .await
        .map_err(|e| ApiError::internal("list the displays", e))?;
    let now = admin.enrollment.now();
    Ok(Json(DisplayList {
        displays: pairings
            .iter()
            .map(|pairing| DisplayEntry::of(pairing, now, admin.enrollment.certificate_lifetime()))
            .collect(),
    }))
}

/// One display and where it stands.
#[utoipa::path(
    get,
    path = "/v1/displays/{name}",
    tag = "displays",
    params(DisplayPath),
    responses(
        (status = 200, description = "The display.", body = DisplayEntry),
        (status = 404, description = "No display of that name is known.", body = ApiError),
        BadRequest,
        ServerFailure
    )
)]
async fn show_display(
    State(admin): State<Arc<Admin>>,
    Path(DisplayPath { name }): Path<DisplayPath>,
) -> Result<Json<DisplayEntry>, ApiError> {
    Ok(Json(entry_of(&admin, &device_id(&name)?).await?))
}

/// Forgets a display, whatever its state, so it can ask again as if for the first time. A member that is forgotten
/// is turned away at once, whatever its certificate says.
#[utoipa::path(
    delete,
    path = "/v1/displays/{name}",
    tag = "displays",
    params(DisplayPath),
    responses(
        (status = 200, description = "The display as it was, now forgotten.", body = DisplayEntry),
        (status = 404, description = "No display of that name is known.", body = ApiError),
        BadRequest,
        ServerFailure
    )
)]
async fn forget_display(
    State(admin): State<Arc<Admin>>,
    Path(DisplayPath { name }): Path<DisplayPath>,
) -> Result<Json<DisplayEntry>, ApiError> {
    let device = device_id(&name)?;
    let before = entry_of(&admin, &device).await?;
    admin
        .enrollment
        .forget(&device)
        .await
        .map_err(|e| ApiError::internal("forget the display", e))?;
    Ok(Json(before))
}

/// Approves a display that is waiting, because the code the owner read off its own panel matches.
#[utoipa::path(
    post,
    path = "/v1/displays/{name}/approve",
    tag = "displays",
    params(DisplayPath),
    request_body(content = ApproveRequest, description = "The code from the display's own panel."),
    responses(
        (status = 200, description = "Approved. The display becomes a member the next time it asks.", body = DisplayEntry),
        (status = 403, description = "The code does not match what the display would show. The display stays as it was.", body = ApiError),
        (status = 404, description = "No display of that name is waiting, or its request lapsed.", body = ApiError),
        (status = 409, description = "The display is not waiting for approval.", body = ApiError),
        (status = 429, description = "Too many wrong codes were typed for the display in a row, so none is looked at for 15 minutes, the right one included. The message says for how long.", body = ApiError),
        BadRequest,
        ServerFailure
    )
)]
async fn approve_display(
    State(admin): State<Arc<Admin>>,
    Path(DisplayPath { name }): Path<DisplayPath>,
    body: Result<Json<ApproveRequest>, JsonRejection>,
) -> Result<Json<DisplayEntry>, ApiError> {
    let device = device_id(&name)?;
    let Json(ApproveRequest { code }) = body.map_err(|rejection| {
        ApiError::new(
            ErrorCode::InvalidRequest,
            format!(
                "The body must be JSON like {{\"code\": \"B0AJ-QTW6-Y8SA\"}} ({})",
                rejection.body_text()
            ),
        )
    })?;
    let code = PairingCode::parse(&code)
        .map_err(|e| ApiError::new(ErrorCode::InvalidCode, e.to_string()))?;
    admin
        .enrollment
        .approve(&device, &code)
        .await
        .map_err(|e| refused(e, false))?;
    Ok(Json(entry_of(&admin, &device).await?))
}

/// Turns a waiting display down, until it is forgotten. For a member with another key waiting to take its name,
/// turns that key down and leaves the member as it was.
#[utoipa::path(
    post,
    path = "/v1/displays/{name}/reject",
    tag = "displays",
    params(DisplayPath),
    responses(
        (status = 200, description = "Turned down.", body = DisplayEntry),
        (status = 404, description = "No display of that name is waiting, or its request lapsed.", body = ApiError),
        (status = 409, description = "The display is not waiting for approval.", body = ApiError),
        BadRequest,
        ServerFailure
    )
)]
async fn reject_display(
    State(admin): State<Arc<Admin>>,
    Path(DisplayPath { name }): Path<DisplayPath>,
) -> Result<Json<DisplayEntry>, ApiError> {
    let device = device_id(&name)?;
    admin
        .enrollment
        .reject(&device)
        .await
        .map_err(|e| refused(e, false))?;
    Ok(Json(entry_of(&admin, &device).await?))
}

/// Ends a member's membership. It is turned away on its very next request, whatever its certificate says, and
/// cannot renew.
#[utoipa::path(
    post,
    path = "/v1/displays/{name}/revoke",
    tag = "displays",
    params(DisplayPath),
    responses(
        (status = 200, description = "Revoked.", body = DisplayEntry),
        (status = 404, description = "No display of that name is known.", body = ApiError),
        (status = 409, description = "The display is not a member.", body = ApiError),
        BadRequest,
        ServerFailure
    )
)]
async fn revoke_display(
    State(admin): State<Arc<Admin>>,
    Path(DisplayPath { name }): Path<DisplayPath>,
) -> Result<Json<DisplayEntry>, ApiError> {
    let device = device_id(&name)?;
    admin
        .enrollment
        .revoke(&device)
        .await
        .map_err(|e| refused(e, true))?;
    Ok(Json(entry_of(&admin, &device).await?))
}

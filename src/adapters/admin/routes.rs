//! The admin API's endpoints.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::response::{IntoResponse, Response};
use tower_http::timeout::TimeoutLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::api::{
    ApiError, BadRequest, ErrorBody, ErrorCode, MAX_WINDOW_MINUTES, OpenWindow, ServerFailure,
    WindowState,
};
use crate::application::enrollment::Enrollment;

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
    components(schemas(ApiError, ErrorBody, ErrorCode)),
    tags((name = "pairing", description = "Letting displays join."))
)]
struct ApiDoc;

fn routes() -> OpenApiRouter<Arc<Admin>> {
    let mut description = ApiDoc::openapi();
    description.info.license = None;
    OpenApiRouter::with_openapi(description).routes(routes!(show_window, open_window, close_window))
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

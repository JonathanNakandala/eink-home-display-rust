//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;
mod health;
mod image;
mod metrics;
mod negotiate;
mod plan;
#[cfg(test)]
mod testing;

use std::sync::Arc;

use anyhow::Context;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use self::advertise::Advertisement;
use crate::application::devices::DeviceBoard;
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::published_images::PublishedImages;

pub(super) struct Published {
    pub(super) images: Arc<dyn PublishedImages>,
    pub(super) format: ImageFormat,
    pub(super) schedule: Schedule,
    pub(super) timing: PlanTiming,
    pub(super) handles: Handles,
}

/// How the server listens and announces itself.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    /// Address to listen on.
    pub bind: std::net::SocketAddr,
    /// Announce the server over mDNS / DNS-SD.
    pub advertise: bool,
    /// The name shown for the service in a scan.
    pub instance_name: String,
    /// What `/plan` tells a display about when to come back.
    pub timing: PlanTiming,
}

/// What the render loop shares with the server.
#[derive(Clone)]
pub struct Handles {
    /// Lets a display's button trigger a render.
    pub refresh: Arc<RefreshControl>,
    /// The render history, for `/status` and `/healthz`.
    pub status: Arc<StatusBoard>,
    /// What the displays report about themselves, for `/status` and `/metrics`.
    pub devices: Arc<DeviceBoard>,
}

pub fn router(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    schedule: Schedule,
    timing: PlanTiming,
    handles: Handles,
) -> Router {
    Router::new()
        .route("/image", get(image::image))
        .route("/plan", get(plan::plan))
        .route("/refresh", post(plan::refresh_now))
        .route("/status", get(health::status))
        .route("/healthz", get(health::healthz))
        .route("/metrics", get(health::metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published { images, format, schedule, timing, handles }))
}









pub(super) fn server_error(action: &str, e: impl std::fmt::Display) -> Response {
    log::error!("Failed to {action} the published image: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}


/// Serves until the future is dropped, or fails at once if the address can't be bound.
pub async fn serve(
    settings: &ServerSettings,
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    schedule: Schedule,
    handles: Handles,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(settings.bind)
        .await
        .with_context(|| format!("Failed to listen on {}", settings.bind))?;
    log::info!("Serving the display image at http://{}/image", settings.bind);
    // Discovery is a convenience, so the server runs without it. Held until serving ends,
    // which is when the goodbye goes out.
    let _advertisement = settings
        .advertise
        .then(|| Advertisement::start(settings, listener.local_addr()?.port(), format))
        .transpose()
        .unwrap_or_else(|e| {
            log::warn!("Not advertising over mDNS: {e:#}");
            None
        });
    axum::serve(listener, router(images, format, schedule, settings.timing, handles))
        .await
        .context("Image server stopped")
}

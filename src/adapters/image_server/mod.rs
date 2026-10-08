//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;
mod health;
mod identity;
mod image;
mod metrics;
mod negotiate;
mod plan;
#[cfg(test)]
mod testing;
mod transfer;

use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

pub use self::advertise::{Advertisement, SecureOffer, mdns_host_name};
use crate::adapters::listen::{self, Bound};
use crate::application::devices::DeviceBoard;
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::published_images::PublishedImages;

pub(super) struct Published {
    pub(super) images: Arc<dyn PublishedImages>,
    pub(super) format: ImageFormat,
    pub(super) schedule: Schedule,
    pub(super) timing: PlanTiming,
    pub(super) handles: Handles,
    pub(super) clock: Arc<dyn Clock>,
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
    clock: Arc<dyn Clock>,
) -> Router {
    Router::new()
        .route("/image", get(image::image))
        .route("/plan", get(plan::plan))
        .route("/refresh", post(plan::refresh_now))
        .route("/status", get(health::status))
        .route("/healthz", get(health::healthz))
        .route("/metrics", get(health::metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published {
            images,
            format,
            schedule,
            timing,
            handles,
            clock,
        }))
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
    clock: Arc<dyn Clock>,
) -> anyhow::Result<()> {
    let app = router(images, format, schedule, settings.timing, handles, clock);
    serve_router(settings, app, format, None).await
}

/// Starts announcing the server over mDNS, if `settings` ask for it. Discovery is a convenience, so a
/// failure is a warning and the server runs without it. Held until serving ends, which is when the
/// goodbye goes out.
pub fn announce(
    settings: &ServerSettings,
    port: u16,
    format: ImageFormat,
    families: listen::Families,
    secure: Option<SecureOffer>,
) -> Option<Advertisement> {
    settings
        .advertise
        .then(|| Advertisement::start(settings, port, format, families, secure))
        .transpose()
        .unwrap_or_else(|e| {
            log::warn!("Not advertising over mDNS: {e:#}");
            None
        })
}

/// Serves `app` over plain HTTP at `settings.bind` and announces it, with the HTTPS on offer if any.
pub async fn serve_router(
    settings: &ServerSettings,
    app: Router,
    format: ImageFormat,
    secure: Option<SecureOffer>,
) -> anyhow::Result<()> {
    bind_http(settings)?
        .serve(settings, app, format, secure)
        .await
}

/// The plain-HTTP socket, open and not yet serving, so its address can be read (the port may have
/// been left to the system to choose) before it is.
pub struct HttpListener {
    bound: Bound,
}

pub fn bind_http(settings: &ServerSettings) -> anyhow::Result<HttpListener> {
    Ok(HttpListener {
        bound: listen::bind(settings.bind)?,
    })
}

impl HttpListener {
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.bound.listener.local_addr()
    }

    pub async fn serve(
        self,
        settings: &ServerSettings,
        app: Router,
        format: ImageFormat,
        secure: Option<SecureOffer>,
    ) -> anyhow::Result<()> {
        let Bound { listener, families } = self.bound;
        let port = listener.local_addr()?.port();
        log::info!(
            "Serving the display image at http://{}/image ({families})",
            settings.bind
        );
        // It announces the IP versions the socket really accepts, which isn't always what the
        // configured address says (see `listen`).
        let _advertisement = announce(settings, port, format, families, secure);
        axum::serve(listener, app)
            .await
            .context("Image server stopped")
    }
}

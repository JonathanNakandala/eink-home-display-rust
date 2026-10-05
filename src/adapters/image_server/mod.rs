//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;
mod plan;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, Local};
use serde::Deserialize;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use self::advertise::Advertisement;
pub use self::plan::{render_due, PlanTiming};
use self::plan::compute;
use crate::config::server::ServerConfig;
use crate::domain::models::display::ImageFormat;
use crate::scheduler::Schedule;

fn published_path(directory: &Path, format: ImageFormat) -> PathBuf {
    directory.join(format!("image.{}", format.extension()))
}

/// Makes `bytes` the image that is served. Written beside the target and renamed into
/// place, so a download in progress never sees half a file.
pub async fn publish(directory: &Path, format: ImageFormat, bytes: &[u8]) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(directory)
        .await
        .with_context(|| format!("Failed to create {}", directory.display()))?;
    let target = published_path(directory, format);
    let staging = target.with_extension(format!("{}.tmp", format.extension()));
    tokio::fs::write(&staging, bytes)
        .await
        .with_context(|| format!("Failed to write {}", staging.display()))?;
    tokio::fs::rename(&staging, &target)
        .await
        .with_context(|| format!("Failed to move the image into {}", target.display()))?;
    Ok(())
}

struct Published {
    directory: PathBuf,
    format: ImageFormat,
    schedule: Schedule,
    timing: PlanTiming,
}

/// When the image being served was rendered, or None if there isn't one yet.
pub fn rendered_at(directory: &Path, format: ImageFormat) -> Option<DateTime<Local>> {
    let modified = std::fs::metadata(published_path(directory, format)).ok()?.modified().ok()?;
    Some(DateTime::<Local>::from(modified))
}

pub fn router(directory: PathBuf, format: ImageFormat, schedule: Schedule, timing: PlanTiming) -> Router {
    Router::new()
        .route("/image", get(image))
        .route("/plan", get(plan))
        .route("/healthz", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published { directory, format, schedule, timing }))
}

#[derive(Deserialize)]
struct PlanQuery {
    /// The version of the image the display already shows.
    have: Option<u32>,
}

/// Tells a display which render the image is, whether it is stale, and when to ask again.
async fn plan(State(published): State<Arc<Published>>, Query(query): Query<PlanQuery>) -> Response {
    let path = published_path(&published.directory, published.format);
    let modified = match tokio::fs::metadata(&path).await.and_then(|metadata| metadata.modified()) {
        Ok(modified) => modified,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response();
        }
        Err(e) => return server_error("inspect", e),
    };
    match compute(Local::now(), DateTime::<Local>::from(modified), &published.schedule, published.timing, query.have) {
        Ok(plan) => ([(header::CACHE_CONTROL, "no-store")], Json(plan)).into_response(),
        Err(e) => server_error("plan", e),
    }
}

fn server_error(action: &str, e: impl std::fmt::Display) -> Response {
    log::error!("Failed to {action} the published image: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

async fn image(State(published): State<Arc<Published>>) -> impl IntoResponse {
    match tokio::fs::read(published_path(&published.directory, published.format)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, published.format.content_type()),
                // The picture changes every refresh, so nothing may reuse an old one.
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response()
        }
        Err(e) => {
            log::error!("Failed to read the published image: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Serves until the future is dropped, or fails at once if the address can't be bound.
pub async fn serve(config: &ServerConfig, format: ImageFormat, schedule: Schedule) -> anyhow::Result<()> {
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("Failed to listen on {}", config.bind))?;
    log::info!("Serving the display image at http://{}/image", config.bind);
    // Discovery is a convenience, so the server runs without it. Held until serving ends,
    // which is when the goodbye goes out.
    let _advertisement = config
        .advertise
        .then(|| Advertisement::start(config, listener.local_addr()?.port(), format))
        .transpose()
        .unwrap_or_else(|e| {
            log::warn!("Not advertising over mDNS: {e:#}");
            None
        });
    axum::serve(listener, router(config.directory.clone(), format, schedule, PlanTiming::from(config)))
        .await
        .context("Image server stopped")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(directory: PathBuf, format: ImageFormat) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let timing = PlanTiming::from(&ServerConfig::default());
        let schedule = Schedule::parse_every("1h").unwrap();
        tokio::spawn(async move { axum::serve(listener, router(directory, format, schedule, timing)).await });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn serves_the_published_image_with_its_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        assert_eq!(reqwest::get(format!("{base}/image")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, &[1, 2, 3]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/bmp");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.bytes().await.unwrap().as_ref(), [1, 2, 3]);

        // A new publish replaces it and leaves no staging file behind.
        publish(tmp.path(), ImageFormat::Bmp, &[9]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.bytes().await.unwrap().as_ref(), [9]);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn healthz_answers_without_an_image() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        assert_eq!(reqwest::get(format!("{base}/healthz")).await.unwrap().status(), 200);
    }

    fn set_age(directory: &Path, seconds: u64) {
        let file = std::fs::File::options().write(true).open(directory.join("image.bmp")).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)).unwrap();
    }

    #[tokio::test]
    async fn plan_reports_the_version_and_when_to_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        assert_eq!(reqwest::get(format!("{base}/plan")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 100);
        let plan: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        let version = plan["version"].as_u64().unwrap();
        let expected = rendered_at(tmp.path(), ImageFormat::Bmp).unwrap().timestamp() as u64;
        assert_eq!(version, expected);
        assert_eq!(plan["changed"], true);
        assert_eq!(plan["stale"], false);
        // An hour after the render, less the 100 seconds already gone, plus the 30 second delay.
        let next = plan["next_seconds"].as_u64().unwrap();
        assert!((3525..=3535).contains(&next), "{next}");

        let same: serde_json::Value =
            reqwest::get(format!("{base}/plan?have={version}")).await.unwrap().json().await.unwrap();
        assert_eq!(same["changed"], false);
        let other: serde_json::Value = reqwest::get(format!("{base}/plan?have=1")).await.unwrap().json().await.unwrap();
        assert_eq!(other["changed"], true);

        // A newer render is a larger version.
        publish(tmp.path(), ImageFormat::Bmp, b"b").await.unwrap();
        let newer: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        assert!(newer["version"].as_u64().unwrap() >= version + 99);
    }

    #[tokio::test]
    async fn an_old_image_is_reported_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 3 * 3600);

        let plan: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        assert_eq!(plan["stale"], true);
        assert!(plan["age_seconds"].as_u64().unwrap() >= 3 * 3600);
    }
}

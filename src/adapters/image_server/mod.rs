//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use self::advertise::Advertisement;
use crate::config::server::ServerConfig;
use crate::domain::models::display::ImageFormat;

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

#[derive(Clone)]
struct Published {
    directory: PathBuf,
    format: ImageFormat,
}

pub fn router(directory: PathBuf, format: ImageFormat) -> Router {
    Router::new()
        .route("/image", get(image))
        .route("/healthz", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published { directory, format }))
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
pub async fn serve(config: &ServerConfig, format: ImageFormat) -> anyhow::Result<()> {
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
    axum::serve(listener, router(config.directory.clone(), format))
        .await
        .context("Image server stopped")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(directory: PathBuf, format: ImageFormat) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(directory, format)).await });
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
}

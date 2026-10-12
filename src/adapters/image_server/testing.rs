//! Helpers for the route tests: a server on a free port over a temporary directory.

use chrono_tz::Europe::London;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use super::{Handles, router};
use crate::adapters::clock::SystemClock;
use crate::adapters::published_images::DirectoryImages;
use crate::application::devices::DeviceBoard;
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{DEFAULT_LAYOUT, Layout, LayoutName, Layouts};
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::published_images::PublishedImages;

pub(super) async fn publish(
    directory: &Path,
    format: ImageFormat,
    bytes: &[u8],
) -> anyhow::Result<()> {
    publish_layout(directory, DEFAULT_LAYOUT, format, bytes).await
}

/// Publishes the image of the layout called `layout`.
pub(super) async fn publish_layout(
    directory: &Path,
    layout: &str,
    format: ImageFormat,
    bytes: &[u8],
) -> anyhow::Result<()> {
    DirectoryImages::new(directory)
        .publish(&LayoutName::parse(layout).unwrap(), format, bytes)
        .await
}

/// A server that has two layouts, the default and `departures`.
pub(super) async fn start_with_two_layouts(directory: PathBuf, format: ImageFormat) -> String {
    let layouts = Layouts::new(vec![
        Layout::new(DEFAULT_LAYOUT, "Everything"),
        Layout::new("departures", "Just the trains"),
    ])
    .unwrap();
    start_layouts(
        Arc::new(DirectoryImages::new(directory)),
        format,
        Arc::new(SystemClock::new(London)),
        None,
        layouts,
    )
    .await
    .0
}

pub(super) async fn start(directory: PathBuf, format: ImageFormat) -> String {
    start_with_status(directory, format).await.0
}

/// Also hands back the status board, to play the render loop's part.
pub(super) async fn start_with_status(
    directory: PathBuf,
    format: ImageFormat,
) -> (String, Arc<StatusBoard>) {
    start_with_clock(directory, format, Arc::new(SystemClock::new(London))).await
}

/// Like `start_with_status`, with the server reading the time from `clock`.
pub(super) async fn start_with_clock(
    directory: PathBuf,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
) -> (String, Arc<StatusBoard>) {
    start_with(Arc::new(DirectoryImages::new(directory)), format, clock).await
}

/// Like `start_with_clock`, over any store of images (to make one misbehave).
pub(super) async fn start_with(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
) -> (String, Arc<StatusBoard>) {
    start_full(images, format, clock, None).await
}

/// Like `start_with`, reporting on the displays in the certificate authority too.
pub(super) async fn start_full(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
    members: Option<Arc<crate::application::enrollment::Enrollment>>,
) -> (String, Arc<StatusBoard>) {
    start_layouts(images, format, clock, members, Layouts::default()).await
}

/// Like `start_full`, with the layouts there are.
pub(super) async fn start_layouts(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
    members: Option<Arc<crate::application::enrollment::Enrollment>>,
    layouts: Layouts,
) -> (String, Arc<StatusBoard>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let timing = PlanTiming {
        wake_delay: Duration::from_secs(30),
        stale_grace: Duration::from_secs(300),
    };
    let schedule = Schedule::parse_every("1h").unwrap();
    let status = StatusBoard::new(chrono::Utc::now().with_timezone(&London));
    let handles = Handles {
        layouts,
        refresh: RefreshControl::new(Duration::from_secs(30)),
        status: Arc::clone(&status),
        devices: DeviceBoard::new(Duration::from_secs(900)),
        members,
        handshakes: None,
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            router(images, format, schedule, timing, handles, clock),
        )
        .await
    });
    (format!("http://{address}"), status)
}

pub(super) fn set_age(directory: &Path, seconds: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(directory.join(DEFAULT_LAYOUT).join("image.bmp"))
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

/// A directory of images where one format fails to be inspected, and one fails to be read.
pub(super) struct Faulty {
    pub(super) inner: DirectoryImages,
    pub(super) cannot_inspect: Option<ImageFormat>,
    pub(super) cannot_read: Option<ImageFormat>,
}

#[async_trait::async_trait]
impl PublishedImages for Faulty {
    async fn publish(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        self.inner.publish(layout, format, bytes).await
    }

    async fn published_at(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<chrono::DateTime<chrono::Utc>>> {
        anyhow::ensure!(self.cannot_inspect != Some(format), "permission denied");
        self.inner.published_at(layout, format).await
    }

    async fn read(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        anyhow::ensure!(self.cannot_read != Some(format), "input/output error");
        self.inner.read(layout, format).await
    }
}

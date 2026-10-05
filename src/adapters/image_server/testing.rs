//! Helpers for the route tests: a server on a free port over a temporary directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use tokio::net::TcpListener;

use super::{router, Handles};
use crate::adapters::published_images::DirectoryImages;
use crate::application::devices::DeviceBoard;
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::published_images::PublishedImages;

pub(super) async fn publish(directory: &Path, format: ImageFormat, bytes: &[u8]) -> anyhow::Result<()> {
    DirectoryImages::new(directory).publish(format, bytes).await
}

pub(super) async fn start(directory: PathBuf, format: ImageFormat) -> String {
    start_with_status(directory, format).await.0
}

/// Also hands back the status board, to play the render loop's part.
pub(super) async fn start_with_status(directory: PathBuf, format: ImageFormat) -> (String, Arc<StatusBoard>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let timing = PlanTiming { wake_delay: Duration::from_secs(30), stale_grace: Duration::from_secs(300) };
    let schedule = Schedule::parse_every("1h").unwrap();
    let status = StatusBoard::new(Local::now());
    let handles = Handles { refresh: RefreshControl::new(Duration::from_secs(30)), status: Arc::clone(&status), devices: DeviceBoard::new(Duration::from_secs(900)) };
    tokio::spawn(async move { axum::serve(listener, router(Arc::new(DirectoryImages::new(directory)), format, schedule, timing, handles)).await });
    (format!("http://{address}"), status)
}

pub(super) fn set_age(directory: &Path, seconds: u64) {
    let file = std::fs::File::options().write(true).open(directory.join("image.bmp")).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)).unwrap();
}

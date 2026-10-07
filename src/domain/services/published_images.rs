use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::models::display::ImageFormat;

/// Where the rendered image is made available to displays that fetch it, one copy per format.
///
/// A render's version is when its file was written, so whoever publishes several formats writes
/// the preferred one last.
#[async_trait]
pub trait PublishedImages: Send + Sync {
    /// Makes `bytes` the image served in `format`. A reader never sees a half-written image.
    async fn publish(&self, format: ImageFormat, bytes: &[u8]) -> anyhow::Result<()>;

    /// When the image in `format` was published, or None if there isn't one yet. An instant, with no zone:
    /// the caller reads it in whichever zone it is working in.
    async fn published_at(&self, format: ImageFormat) -> anyhow::Result<Option<DateTime<Utc>>>;

    /// The image in `format`, or None if there isn't one (or it was just replaced).
    async fn read(&self, format: ImageFormat) -> anyhow::Result<Option<Vec<u8>>>;
}

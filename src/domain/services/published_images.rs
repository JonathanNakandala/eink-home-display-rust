use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::LayoutName;

/// Where the rendered images are made available to displays that fetch them, one copy per layout and format.
///
/// A render's version is when its file was written, so whoever publishes several formats writes
/// the preferred one last.
#[async_trait]
pub trait PublishedImages: Send + Sync {
    /// Makes `bytes` the image of `layout` served in `format`. A reader never sees a half-written image.
    async fn publish(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
        bytes: &[u8],
    ) -> anyhow::Result<()>;

    /// When the image of `layout` in `format` was published, or None if there isn't one yet. An instant, with no
    /// zone: the caller reads it in whichever zone it is working in.
    async fn published_at(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<DateTime<Utc>>>;

    /// The image of `layout` in `format`, or None if there isn't one (or it was just replaced).
    async fn read(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<Vec<u8>>>;
}

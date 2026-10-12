use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;

#[async_trait::async_trait]
pub trait ImageRepository {
    /// Keeps the rendered picture of `layout`.
    async fn store(&self, layout: &LayoutName, image_data: &ImageData) -> anyhow::Result<()>;
}

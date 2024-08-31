use crate::domain::models::image::ImageData;

#[async_trait::async_trait]
pub trait ImageRepository {
    async fn store(&self, image_data: &ImageData) -> anyhow::Result<()>;
}
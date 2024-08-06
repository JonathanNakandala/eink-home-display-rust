use async_trait::async_trait;

use crate::domain::models::image::ImageData;

#[async_trait]
pub trait ImageDisplayService {
    async fn display(&self, data: &ImageData) -> anyhow::Result<()>;
}

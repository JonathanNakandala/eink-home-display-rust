use crate::domain::models::GlanceData;
use crate::domain::models::image::ImageData;

#[allow(async_fn_in_trait)]
pub trait DisplayImageGenerator {
    async fn generate(&self, data: GlanceData) -> anyhow::Result<ImageData>;
}

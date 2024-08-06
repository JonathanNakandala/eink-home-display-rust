use async_trait::async_trait;

use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;

#[derive(derive_new::new)]
pub struct EinkWaveshareAdapter {}

#[async_trait]
impl ImageDisplayService for EinkWaveshareAdapter {
    async fn display(&self, _data: &ImageData) -> anyhow::Result<()> {
        log::debug!("Stubbed EinkWaveshare display");

        Ok(())
    }
}

pub mod eink_waveshare;
pub mod quantise;

use async_trait::async_trait;

use crate::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use crate::config::application::{DisplayConfig, DisplayKind};
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;

pub enum DisplayImpl {
    WaveshareEpd7in5V2(EinkWaveshareAdapter),
}

#[async_trait]
impl ImageDisplayService for DisplayImpl {
    fn profile(&self) -> DisplayProfile {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.profile(),
        }
    }

    async fn display(&self, data: &ImageData) -> anyhow::Result<()> {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.display(data).await,
        }
    }
}

pub fn setup_display(config: &DisplayConfig) -> DisplayImpl {
    match config.kind {
        DisplayKind::WaveshareEpd7in5V2 => {
            DisplayImpl::WaveshareEpd7in5V2(EinkWaveshareAdapter::new(config.dither))
        }
    }
}

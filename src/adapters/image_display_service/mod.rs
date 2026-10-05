pub mod eink_waveshare;
pub mod quantise;
pub mod reterminal_e1003;

use std::sync::Arc;

use async_trait::async_trait;

use crate::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use crate::adapters::image_display_service::reterminal_e1003::ReTerminalE1003Adapter;
use crate::config::application::{DisplayConfig, DisplayKind};
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;
use crate::domain::services::published_images::PublishedImages;

pub enum DisplayImpl {
    WaveshareEpd7in5V2(EinkWaveshareAdapter),
    ReTerminalE1003(ReTerminalE1003Adapter),
}

#[async_trait]
impl ImageDisplayService for DisplayImpl {
    fn profile(&self) -> DisplayProfile {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.profile(),
            DisplayImpl::ReTerminalE1003(display) => display.profile(),
        }
    }

    async fn display(&self, data: &ImageData) -> anyhow::Result<()> {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.display(data).await,
            DisplayImpl::ReTerminalE1003(display) => display.display(data).await,
        }
    }
}

/// `images` is where displays that fetch their image have it published.
pub fn setup_display(config: &DisplayConfig, images: Arc<dyn PublishedImages>) -> DisplayImpl {
    match config.kind {
        DisplayKind::WaveshareEpd7in5V2 => {
            DisplayImpl::WaveshareEpd7in5V2(EinkWaveshareAdapter::new(config.dither))
        }
        DisplayKind::ReTerminalE1003 => DisplayImpl::ReTerminalE1003(ReTerminalE1003Adapter::new(
            config.dither,
            config.image_format,
            images,
        )),
    }
}

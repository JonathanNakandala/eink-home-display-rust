use std::sync::Arc;

use crate::adapters::image_display_service::DisplayImpl;
use crate::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use crate::adapters::image_display_service::reterminal_e1003::ReTerminalE1003Adapter;
use crate::config::application::{DisplayConfig, DisplayKind};
use crate::domain::services::published_images::PublishedImages;

/// `images` is where displays that fetch their image have it published.
pub fn setup_display(config: &DisplayConfig, images: Arc<dyn PublishedImages>) -> DisplayImpl {
    match config.kind {
        DisplayKind::WaveshareEpd7in5V2 => {
            DisplayImpl::WaveshareEpd7in5V2(EinkWaveshareAdapter::new(config.dither.into()))
        }
        DisplayKind::ReTerminalE1003 => DisplayImpl::ReTerminalE1003(ReTerminalE1003Adapter::new(
            config.dither.into(),
            config.image_format.into(),
            images,
        )),
    }
}

pub mod eink_waveshare;
pub mod quantise;
pub mod reterminal_e1003;

use async_trait::async_trait;

use crate::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use crate::adapters::image_display_service::reterminal_e1003::ReTerminalE1003Adapter;
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::image_display_service::ImageDisplayService;

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

    fn wants_every_layout(&self) -> bool {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.wants_every_layout(),
            DisplayImpl::ReTerminalE1003(display) => display.wants_every_layout(),
        }
    }

    async fn display(&self, layout: &LayoutName, data: &ImageData) -> anyhow::Result<()> {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.display(layout, data).await,
            DisplayImpl::ReTerminalE1003(display) => display.display(layout, data).await,
        }
    }
}

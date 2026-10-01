use async_trait::async_trait;

use crate::domain::models::display::{DisplayProfile, Palette};
use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;

/// Seeed reTerminal E1003: 1872x1404, 16 greys. Only the size and colour depth
/// are known so far, enough to render for it; sending to the device is not built yet.
#[derive(derive_new::new)]
pub struct ReTerminalE1003Adapter {}

#[async_trait]
impl ImageDisplayService for ReTerminalE1003Adapter {
    fn profile(&self) -> DisplayProfile {
        DisplayProfile { width: 1872, height: 1404, palette: Palette::Grey(16) }
    }

    async fn display(&self, _data: &ImageData) -> anyhow::Result<()> {
        anyhow::bail!("Showing images on the reTerminal E1003 is not implemented yet")
    }
}

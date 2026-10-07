use async_trait::async_trait;

use crate::domain::models::display::{DisplayProfile, Dither, Palette};
use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;

use super::eink_driver::{HEIGHT, WIDTH};
use super::frame::frame_from_encoded_image;

/// Waveshare 7.5" V2: 800x480, black and white.
#[derive(derive_new::new)]
pub struct EinkWaveshareAdapter {
    dither: Dither,
}

#[async_trait]
impl ImageDisplayService for EinkWaveshareAdapter {
    fn profile(&self) -> DisplayProfile {
        DisplayProfile {
            width: WIDTH,
            height: HEIGHT,
            palette: Palette::Mono,
        }
    }

    async fn display(&self, data: &ImageData) -> anyhow::Result<()> {
        let frame = frame_from_encoded_image(&data.data, self.dither)?;

        // SPI and the panel's busy-wait are blocking, and a refresh takes seconds.
        tokio::task::spawn_blocking(move || show_frame(&frame)).await?
    }
}

#[cfg(target_os = "linux")]
fn show_frame(frame: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;

    let mut hardware = super::hardware::PoweredPanel::open()?;
    let panel = &mut hardware.panel;
    panel.init().context("Failed to initialise e-paper panel")?;
    panel.display(frame).context("Failed to display frame")?;
    panel
        .sleep()
        .context("Failed to put e-paper panel to sleep")?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn show_frame(frame: &[u8]) -> anyhow::Result<()> {
    log::warn!(
        "No e-paper hardware on this platform; skipping display of {} byte frame",
        frame.len()
    );
    Ok(())
}

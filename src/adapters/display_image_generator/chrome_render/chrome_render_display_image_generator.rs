use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use handlebars::Handlebars;
use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::browser::default_executable;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions};

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Renders the dashboard template in headless Chrome at the display's native
/// size and returns the screenshot. Reducing it to the panel's colour depth is
/// the display adapter's job.
#[derive(derive_new::new)]
pub struct ChromeRenderDisplayImageGenerator {}

fn find_system_chrome() -> Option<PathBuf> {
    let path = match default_executable() {
        Ok(path) => path,
        Err(reason) => {
            log::info!("No system Chrome found ({reason}); will use a downloaded one");
            return None;
        }
    };
    match Command::new(&path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            log::info!("Found system Chrome at {}: {}", path.display(), version.trim());
            Some(path)
        }
        Ok(output) => {
            log::warn!("{} --version failed ({}); ignoring it", path.display(), output.status);
            None
        }
        Err(e) => {
            log::warn!("Could not run {}: {e}; ignoring it", path.display());
            None
        }
    }
}

impl ChromeRenderDisplayImageGenerator {
    fn render_glance_data(&self, glance_data: &GlanceData) -> anyhow::Result<String> {
        let mut handlebars = Handlebars::new();
        handlebars.register_template_string(
            "dashboard",
            include_str!("../../../../templates/dashboard.html"),
        )?;

        let rendered = handlebars.render("dashboard", glance_data)?;
        Ok(rendered)
    }

    async fn capture_webpage(&self, html: String, profile: &DisplayProfile) -> anyhow::Result<Vec<u8>> {
        let fetcher_options = FetcherOptions::default()
            .with_allow_download(true)
            .with_install_dir("/tmp/headless_chrome".into());
        let launcher_options = LaunchOptions::default_builder()
            .window_size(Some((profile.width, profile.height)))
            .devtools(false)
            .headless(true)
            // None makes headless_chrome download its pinned revision instead.
            .path(find_system_chrome())
            .fetcher_options(fetcher_options)
            .build()?;
        let browser =
            Browser::new(launcher_options).context("Failed to create browser instance")?;
        match browser.get_version() {
            Ok(v) => log::info!("Rendering with {} (protocol {})", v.product, v.protocol_version),
            Err(e) => log::warn!("Could not read Chrome version: {e}"),
        }
        browser.set_default_timeout(RENDER_TIMEOUT);
        let tab = browser.new_tab().context("Failed to create new tab")?;
        tab.set_default_timeout(RENDER_TIMEOUT);
        // window_size covers the window, not the page, and a HiDPI host would double it.
        // Pin the viewport to exactly one CSS pixel per panel pixel.
        tab.call_method(SetDeviceMetricsOverride {
            width: profile.width,
            height: profile.height,
            device_scale_factor: 1.0,
            mobile: false,
            scale: None,
            screen_width: None,
            screen_height: None,
            position_x: None,
            position_y: None,
            dont_set_visible_size: None,
            screen_orientation: None,
            viewport: None,
            display_feature: None,
            device_posture: None,
        })
        .context("Failed to set viewport size")?;
        tab.navigate_to(format!("data:text/html;charset=utf-8,{}", html).as_str())?
            .wait_until_navigated()?;

        let screenshot_options = CaptureScreenshotFormatOption::Png;
        let png_data = tab
            .capture_screenshot(screenshot_options, None, None, true)
            .context("Failed to capture screenshot")?;

        Ok(png_data)
    }
}

impl DisplayImageGenerator for ChromeRenderDisplayImageGenerator {
    async fn generate(
        &self,
        data: GlanceData,
        profile: &DisplayProfile,
    ) -> anyhow::Result<ImageData> {
        let html = self.render_glance_data(&data)?;
        let png = self.capture_webpage(html, profile).await?;
        Ok(ImageData::new(png))
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::models::display::Palette;
    use crate::domain::models::weather::WeatherInformation;

    use super::*;

    /// Needs a real Chrome (and network for the font), so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn renders_a_png_at_the_profile_size() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile { width: 800, height: 480, palette: Palette::Mono };
        let data = GlanceData::new(WeatherInformation::new(12), vec![]);

        let image = ChromeRenderDisplayImageGenerator::new()
            .generate(data, &profile)
            .await
            .unwrap();

        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
    }
}

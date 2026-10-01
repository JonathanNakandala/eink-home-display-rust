use anyhow::Context;
use handlebars::Handlebars;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions};

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

/// Renders the dashboard template in headless Chrome at the display's native
/// size and returns the screenshot. Reducing it to the panel's colour depth is
/// the display adapter's job.
#[derive(derive_new::new)]
pub struct ChromeRenderDisplayImageGenerator {}

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
            .fetcher_options(fetcher_options)
            .build()?;
        let browser =
            Browser::new(launcher_options).context("Failed to create browser instance")?;
        let tab = browser.new_tab().context("Failed to create new tab")?;
        tab.navigate_to(format!("data:text/html;charset=utf-8,{}", html).as_str())?
            .wait_until_navigated()?;

        let screenshot_options = CaptureScreenshotFormatOption::Png;
        let png_data = tab
            .capture_screenshot(screenshot_options, None, None, false)
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

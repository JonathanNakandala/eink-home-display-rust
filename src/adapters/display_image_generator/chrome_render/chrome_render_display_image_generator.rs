use std::path::{Path, PathBuf};

use anyhow::Context;
use handlebars::Handlebars;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions};
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::adapters::display_image_generator::chrome_render::generate_1bit::{
    generate_1bit_image, save_1bit_array_as_png,
};
use crate::domain::models::GlanceData;
use crate::domain::models::image::ImageData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

#[derive(derive_new::new)]
pub struct ChromeRenderDisplayImageGenerator {
    image_width: u32,
    image_height: u32,
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

    async fn capture_webpage(&self, html: String) -> anyhow::Result<Vec<u8>> {
        let fetcher_options = FetcherOptions::default()
            .with_allow_download(true)
            .with_install_dir("/tmp/headless_chrome".into());
        let launcher_options = LaunchOptions::default_builder()
            .window_size(Some((self.image_width, self.image_height)))
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

    async fn read_image_from_file(&self, path: impl AsRef<Path>) -> anyhow::Result<Vec<u8>> {
        let mut f = File::open(path).await?;
        let mut buffer = vec![];
        f.read_to_end(&mut buffer).await?;
        Ok(buffer)
    }
}

impl DisplayImageGenerator for ChromeRenderDisplayImageGenerator {
    async fn generate(&self, data: GlanceData) -> anyhow::Result<ImageData> {
        let html = self.render_glance_data(&data)?;

        let rendered_image = self.capture_webpage(html).await?;
        let one_bit_rendered_image =
            generate_1bit_image(rendered_image).context("Failed to generate 1-bit image")?;

        let dir = PathBuf::from("/tmp/eink_display/");

        if !dir.exists() {
            tokio::fs::create_dir(dir.as_path()).await?;
        }

        let output_path = dir.as_path().join("one_bit_screenshot.png");
        save_1bit_array_as_png(
            &one_bit_rendered_image,
            self.image_width,
            self.image_height,
            output_path.as_path(),
        )
        .context("Failed to save 1-bit screenshot")?;

        Ok(ImageData::new(
            self.read_image_from_file(output_path.as_path()).await?,
        ))
    }
}

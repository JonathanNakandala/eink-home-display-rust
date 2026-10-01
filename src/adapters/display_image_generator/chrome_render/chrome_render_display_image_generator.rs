use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Context;
use handlebars::Handlebars;
use url::Url;
use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::browser::default_executable;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions};

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

const DASHBOARD_TEMPLATE: &str = include_str!("../../../../templates/dashboard.html");

/// Files the template references by relative path. They are compiled in and
/// written next to the rendered HTML, so the binary needs nothing deployed
/// alongside it.
const ASSETS: &[(&str, &[u8])] = &[
    (
        "fonts/Overpass/Overpass-Light.ttf",
        include_bytes!("../../../../templates/fonts/Overpass/Overpass-Light.ttf"),
    ),
    (
        "fonts/Overpass/Overpass-SemiBold.ttf",
        include_bytes!("../../../../templates/fonts/Overpass/Overpass-SemiBold.ttf"),
    ),
];

/// Inline SVGs, available in the template as partials named after `WeatherCondition`.
const ICONS: &[(&str, &str)] = &[
    ("clear", include_str!("../../../../templates/weather_icons/013-sun-8.svg")),
    ("clouds", include_str!("../../../../templates/weather_icons/051-cloud-3.svg")),
    ("drizzle", include_str!("../../../../templates/weather_icons/099-rain-4.svg")),
    ("rain", include_str!("../../../../templates/weather_icons/067-storm-6.svg")),
    ("thunderstorm", include_str!("../../../../templates/weather_icons/057-storm-7.svg")),
    ("snow", include_str!("../../../../templates/weather_icons/047-snow-4.svg")),
];

/// Renders the dashboard template in headless Chrome at the display's native
/// size and returns the screenshot. Reducing it to the panel's colour depth is
/// the display adapter's job.
#[derive(derive_new::new)]
pub struct ChromeRenderDisplayImageGenerator {
    chrome_install_dir: PathBuf,
}

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
    /// The dashboard as HTML. Fonts are referenced by relative path, so it
    /// renders correctly from a directory holding [`ASSETS`].
    pub fn render_html(&self, glance_data: &GlanceData) -> anyhow::Result<String> {
        let mut handlebars = Handlebars::new();
        for (name, svg) in ICONS {
            // Drop the XML prolog and comments: they are invalid inside HTML.
            let svg = &svg[svg.find("<svg").unwrap_or(0)..];
            handlebars.register_partial(name, svg)?;
        }
        handlebars.register_template_string("dashboard", DASHBOARD_TEMPLATE)?;
        Ok(handlebars.render("dashboard", glance_data)?)
    }

    /// Writes the HTML and its assets to `dir`, returning the HTML file's path.
    pub fn write_page(&self, dir: &Path, html: &str) -> anyhow::Result<PathBuf> {
        for (path, bytes) in ASSETS {
            let target = dir.join(path);
            std::fs::create_dir_all(target.parent().expect("asset paths have a parent"))?;
            std::fs::write(target, bytes)?;
        }
        let page = dir.join("dashboard.html");
        std::fs::write(&page, html)?;
        Ok(page)
    }

    async fn capture_webpage(&self, page: &Path, profile: &DisplayProfile) -> anyhow::Result<Vec<u8>> {
        let fetcher_options = FetcherOptions::default()
            .with_allow_download(true)
            .with_install_dir(Some(self.chrome_install_dir.clone()));
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
        let url = file_url(page)?;
        tab.navigate_to(url.as_str())?.wait_until_navigated()?;
        // Fonts load after navigation; a screenshot taken before then uses the fallback.
        tab.evaluate("document.fonts.ready.then(() => true)", true)
            .context("Failed to wait for fonts")?;

        let screenshot_options = CaptureScreenshotFormatOption::Png;
        let png_data = tab
            .capture_screenshot(screenshot_options, None, None, true)
            .context("Failed to capture screenshot")?;

        Ok(png_data)
    }
}

/// A `file://` URL for an absolute path, percent-encoding anything (spaces, non-ASCII) that needs it.
fn file_url(path: &Path) -> anyhow::Result<Url> {
    Url::from_file_path(path)
        .map_err(|()| anyhow::anyhow!("{} is not an absolute path", path.display()))
}

impl DisplayImageGenerator for ChromeRenderDisplayImageGenerator {
    async fn generate(
        &self,
        data: GlanceData,
        profile: &DisplayProfile,
    ) -> anyhow::Result<ImageData> {
        let html = self.render_html(&data)?;
        // Keep the directory alive until the screenshot has been taken.
        let dir = tempfile::tempdir().context("Failed to create render directory")?;
        let page = self.write_page(dir.path(), &html)?;
        let png = self.capture_webpage(&page, profile).await?;
        Ok(ImageData::new(png))
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::models::DateInfo;
    use crate::domain::models::display::Palette;
    use crate::domain::models::weather::{WeatherCondition, WeatherInformation};

    use super::*;

    #[test]
    fn file_urls_encode_characters_that_need_it() {
        let url = file_url(Path::new("/tmp/my render/dashboard é.html")).unwrap();

        assert_eq!(url.as_str(), "file:///tmp/my%20render/dashboard%20%C3%A9.html");
    }

    #[test]
    fn file_urls_need_an_absolute_path() {
        assert!(file_url(Path::new("dashboard.html")).is_err());
    }

    /// Needs a real Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn renders_a_png_at_the_profile_size() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile { width: 800, height: 480, palette: Palette::Mono };
        let data = GlanceData::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![],
            DateInfo::new(chrono::Local::now()));

        let image = ChromeRenderDisplayImageGenerator::new(std::env::temp_dir().join("eink_test_chrome"))
            .generate(data, &profile)
            .await
            .unwrap();

        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
    }
}

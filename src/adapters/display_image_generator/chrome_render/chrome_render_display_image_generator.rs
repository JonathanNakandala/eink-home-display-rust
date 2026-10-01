use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use handlebars::Handlebars;
use url::Url;
use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::browser::default_executable;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions, Tab};

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::GlanceData;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an unused Chrome stays connected. Long-running callers should pass
/// something longer than the gap between renders, or every render relaunches.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Which Chrome to render with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeSource {
    /// An installed Chrome if one works, else the pinned download.
    #[default]
    PreferSystem,
    /// Always the pinned download in the cache, so renders don't vary with the host's Chrome.
    Bundled,
}

/// Renders the dashboard template in headless Chrome at the display's native
/// size and returns the screenshot. Reducing it to the panel's colour depth is
/// the display adapter's job.
///
/// Chrome is launched on the first render and kept for later ones, each in its
/// own tab. If it has died or gone idle, the render is retried once on a fresh
/// launch. It is shut down when the generator is dropped.
pub struct ChromeRenderDisplayImageGenerator {
    inner: Arc<Inner>,
}

struct Inner {
    /// Where a downloaded Chrome is extracted, and found again on later runs.
    chrome_install_dir: PathBuf,
    idle_timeout: Duration,
    source: ChromeSource,
    browser: Mutex<Option<LaunchedBrowser>>,
}

struct LaunchedBrowser {
    browser: Browser,
    /// The window it was launched with, so a different display gets a fresh one.
    window: (u32, u32),
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
    pub fn new(chrome_install_dir: PathBuf, idle_timeout: Duration, source: ChromeSource) -> Self {
        Self {
            inner: Arc::new(Inner {
                chrome_install_dir,
                idle_timeout,
                source,
                browser: Mutex::new(None),
            }),
        }
    }

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
}

impl Inner {
    /// Screenshots `page`, reusing the running Chrome where possible.
    fn capture(&self, page: &Path, profile: &DisplayProfile) -> anyhow::Result<Vec<u8>> {
        let window = (profile.width, profile.height);
        let mut slot = self.browser.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(launched) = slot.as_ref().filter(|l| l.window == window) {
            match capture_in_new_tab(&launched.browser, page, profile) {
                Ok(png) => return Ok(png),
                Err(e) => log::warn!("Render with the running Chrome failed ({e:#}); relaunching"),
            }
        }

        // Drop the old one first so its process is gone before the next starts.
        *slot = None;
        let browser = self.launch(window)?;
        let png = capture_in_new_tab(&browser, page, profile)?;
        *slot = Some(LaunchedBrowser { browser, window });
        Ok(png)
    }

    fn launch(&self, window: (u32, u32)) -> anyhow::Result<Browser> {
        let fetcher_options = FetcherOptions::default()
            .with_allow_download(true)
            .with_install_dir(Some(self.chrome_install_dir.clone()));
        let launcher_options = LaunchOptions::default_builder()
            .window_size(Some(window))
            .devtools(false)
            .headless(true)
            // None makes headless_chrome download its pinned revision instead.
            .path(match self.source {
                ChromeSource::PreferSystem => find_system_chrome(),
                ChromeSource::Bundled => {
                    log::info!("Using the bundled Chrome in {}", self.chrome_install_dir.display());
                    None
                }
            })
            .fetcher_options(fetcher_options)
            .idle_browser_timeout(self.idle_timeout)
            .build()?;
        let browser =
            Browser::new(launcher_options).context("Failed to create browser instance")?;
        match browser.get_version() {
            Ok(v) => log::info!("Rendering with {} (protocol {})", v.product, v.protocol_version),
            Err(e) => log::warn!("Could not read Chrome version: {e}"),
        }
        browser.set_default_timeout(RENDER_TIMEOUT);
        Ok(browser)
    }
}

/// Renders in a tab of its own and always closes it, so a kept Chrome doesn't pile them up.
fn capture_in_new_tab(browser: &Browser, page: &Path, profile: &DisplayProfile) -> anyhow::Result<Vec<u8>> {
    let tab = browser.new_tab().context("Failed to create new tab")?;
    let result = capture_in_tab(&tab, page, profile);
    // Not close(false): the pinned Chromium never answers Target.closeTarget, so that call
    // waits out its timeout and leaves the connection dead, defeating reuse.
    if let Err(e) = tab.close(true) {
        log::warn!("Could not close the render tab: {e}");
    }
    result
}

fn capture_in_tab(tab: &Tab, page: &Path, profile: &DisplayProfile) -> anyhow::Result<Vec<u8>> {
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

    tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
        .context("Failed to capture screenshot")
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
        // headless_chrome blocks, so keep it off the async runtime's threads.
        let inner = Arc::clone(&self.inner);
        let profile = profile.clone();
        let png = tokio::task::spawn_blocking(move || inner.capture(&page, &profile))
            .await
            .context("Render task panicked")??;
        drop(dir);
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

        let image = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("eink_test_chrome"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        )
            .generate(data, &profile)
            .await
            .unwrap();

        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
    }

    fn sample_data() -> GlanceData {
        GlanceData::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![],
            DateInfo::new(chrono::Local::now()),
        )
    }

    /// The process id of the Chrome being kept, if any.
    fn kept_pid(generator: &ChromeRenderDisplayImageGenerator) -> Option<u32> {
        let slot = generator.inner.browser.lock().unwrap();
        slot.as_ref().and_then(|l| l.browser.get_process_id())
    }

    /// Needs a real Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn keeps_one_chrome_across_renders_and_relaunches_when_it_dies() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile { width: 800, height: 480, palette: Palette::Mono };
        let generator = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("eink_test_chrome"),
            Duration::from_secs(300),
            ChromeSource::PreferSystem,
        );

        generator.generate(sample_data(), &profile).await.unwrap();
        let first = kept_pid(&generator).expect("Chrome is kept after a render");
        generator.generate(sample_data(), &profile).await.unwrap();
        assert_eq!(kept_pid(&generator), Some(first), "second render reused it");

        assert!(Command::new("kill").arg("-9").arg(first.to_string()).status().unwrap().success());
        std::thread::sleep(Duration::from_millis(500));
        generator.generate(sample_data(), &profile).await.unwrap();
        let relaunched = kept_pid(&generator).expect("Chrome is kept after relaunch");
        assert_ne!(relaunched, first, "a dead Chrome was replaced");
    }

    /// Needs network on first run to download Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn renders_and_reuses_the_bundled_chrome() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile { width: 800, height: 480, palette: Palette::Mono };
        let generator = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("eink_test_chrome"),
            Duration::from_secs(300),
            ChromeSource::Bundled,
        );

        let started = std::time::Instant::now();
        let image = generator.generate(sample_data(), &profile).await.unwrap();
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
        let first = kept_pid(&generator).expect("Chrome is kept after a render");

        let second_started = std::time::Instant::now();
        generator.generate(sample_data(), &profile).await.unwrap();
        assert_eq!(kept_pid(&generator), Some(first), "second render reused it");
        assert!(
            second_started.elapsed() < Duration::from_secs(15),
            "a reused render took {:?} (first took {:?})",
            second_started.elapsed(),
            started.elapsed()
        );
    }
}

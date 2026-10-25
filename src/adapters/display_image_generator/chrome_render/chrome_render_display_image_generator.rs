use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use handlebars::Handlebars;
use headless_chrome::browser::default_executable;
use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions, Tab};
use url::Url;

use super::Templates;
use crate::domain::models::GlanceData;
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::display_image_generator::DisplayImageGenerator;

const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an unused Chrome stays connected. Long-running callers should pass
/// something longer than the gap between renders, or every render relaunches.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The first layout built in, which is the default unless a schedule or a choice says otherwise.
pub const DASHBOARD: &str = "dashboard";

/// The layouts there are, the default first: what each is called, what it shows, and its template.
pub(super) const LAYOUTS: &[(&str, &str, &str)] = &[
    (
        DASHBOARD,
        "Departures with the weather beside them",
        include_str!("../../../../templates/dashboard.html"),
    ),
    (
        "departures",
        "Departures alone, in larger type",
        include_str!("../../../../templates/departures.html"),
    ),
];

/// Pieces the layouts have in common, available to a template as partials of these names.
const PARTS: &[(&str, &str)] = &[
    (
        "style",
        include_str!("../../../../templates/shared/style.html"),
    ),
    (
        "header",
        include_str!("../../../../templates/shared/header.html"),
    ),
    (
        "departures",
        include_str!("../../../../templates/shared/departures.html"),
    ),
];

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
    (
        "clear",
        include_str!("../../../../templates/weather_icons/clear-day.svg"),
    ),
    (
        "clouds",
        include_str!("../../../../templates/weather_icons/cloudy.svg"),
    ),
    (
        "drizzle",
        include_str!("../../../../templates/weather_icons/drizzle.svg"),
    ),
    (
        "rain",
        include_str!("../../../../templates/weather_icons/rain.svg"),
    ),
    (
        "thunderstorm",
        include_str!("../../../../templates/weather_icons/thunderstorms.svg"),
    ),
    (
        "snow",
        include_str!("../../../../templates/weather_icons/snow.svg"),
    ),
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
    templates: Templates,
}

struct Inner {
    /// Where a downloaded Chrome is extracted, and found again on later runs.
    chrome_install_dir: PathBuf,
    idle_timeout: Duration,
    source: ChromeSource,
    browser: Mutex<Option<LaunchedBrowser>>,
    /// The process id of the running Chrome, or 0. Kept outside the lock above, which a render in
    /// progress holds, so a stuck one can be stopped from outside (see `AbandonGuard`).
    chrome_pid: AtomicU32,
}

/// Stops the Chrome a render is using if the render is given up on while it is still running.
///
/// Dropping the render's future (the run's deadline passed) doesn't stop the blocking call inside it:
/// it keeps the browser lock until Chrome answers, and every later render queues up behind it, each on a
/// thread of the pool that file serving shares. Killing Chrome makes that call fail at once.
struct AbandonGuard {
    inner: Arc<Inner>,
    abandoned: Arc<AtomicBool>,
    armed: bool,
}

impl AbandonGuard {
    fn new(inner: Arc<Inner>, abandoned: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            abandoned,
            armed: true,
        }
    }

    /// The render has ended, one way or the other: leave Chrome alone.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AbandonGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.abandoned.store(true, Ordering::SeqCst);
        let pid = self.inner.chrome_pid.load(Ordering::SeqCst);
        if pid != 0 {
            tracing::warn!(
                "The render was given up on while Chrome (pid {pid}) was still working: stopping it"
            );
            kill_process(pid);
        }
    }
}

#[cfg(unix)]
fn kill_process(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else { return };
    // SAFETY: kill(2) takes two integers and touches no memory. At worst the process has already gone.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process(pid: u32) {
    tracing::warn!(
        "Can't stop Chrome (pid {pid}) on this platform; it will end when its calls time out"
    );
}

struct LaunchedBrowser {
    browser: Browser,
}

/// The window Chrome is launched with: as large as any output is drawn at, so that the one Chrome serves them all. A tab's
/// viewport is set to each picture's own size (see `capture_in_tab`), which a window as large as this never limits.
const WINDOW: (u32, u32) = (3840, 2160);

/// The longest a Chrome may take to say its version. Where `--version` works it prints and exits at once.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

fn find_system_chrome() -> Option<PathBuf> {
    let path = match default_executable() {
        Ok(path) => path,
        Err(reason) => {
            tracing::info!("No system Chrome found ({reason}); will use a downloaded one");
            return None;
        }
    };
    // `chrome.exe --version` prints nothing on Windows: it starts the browser, a whole one with a window, on the user's own
    // profile, and that does not exit. Asked here it hung every render until the user's Chrome happened to be running (which
    // takes the command line and exits), and otherwise left a Chrome window open on their desktop. So it is not asked. The
    // version is logged when the browser is launched ("Rendering with ..."), and a Chrome that cannot start fails there.
    if cfg!(windows) {
        tracing::info!("Found system Chrome at {}", path.display());
        return Some(path);
    }
    let mut command = Command::new(&path);
    command.arg("--version");
    match output_within(command, VERSION_PROBE_TIMEOUT) {
        Ok(version) => {
            tracing::info!("Found system Chrome at {}: {version}", path.display());
            Some(path)
        }
        Err(reason) => {
            tracing::warn!("{} --version {reason}; ignoring it", path.display());
            None
        }
    }
}

/// What `command` prints, if it succeeds within `timeout`. A program still running after that is killed: nothing here is
/// allowed to wait for one for ever, since a render waits on this.
fn output_within(mut command: Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not be run ({e})"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("did not finish in {} s", timeout.as_secs()));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("could not be waited for ({e})"));
            }
        }
    };
    if !status.success() {
        return Err(format!("failed ({status})"));
    }
    let mut printed = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut printed);
    }
    Ok(String::from_utf8_lossy(&printed).trim().to_owned())
}

/// How long a Chrome that failed to launch is given to say why when it is run again.
const EXPLAIN_TIMEOUT: Duration = Duration::from_secs(8);

/// Lines Chrome prints that say nothing about why it did not start: a service has no session bus, sound or GPU, and says so.
const CHROME_NOISE: &[&str] = &[
    "dbus", "DBus", "ALSA", "udev", "gcm", "GCM", "vaapi", "libva",
];

/// The Chrome that was downloaded into `dir` (headless_chrome does not say where it put it): the program called chrome, a few
/// folders down.
fn find_downloaded_chrome(dir: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "chrome.exe"
    } else {
        "chrome"
    };
    let mut folders = vec![(dir.to_owned(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        for entry in std::fs::read_dir(&folder).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 3 {
                folders.push((path, depth + 1));
            } else if path.file_name().is_some_and(|n| n == name) && path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// What `chrome` says when it is run as the server runs it, for the log. Launching reads this and throws it away, so a Chrome that
/// would not start (a sandbox the system refuses, a library missing) failed with a timeout and no reason.
fn explain_launch_failure(chrome: &Path, timeout: Duration) -> String {
    let profile = match tempfile::tempdir() {
        Ok(profile) => profile,
        Err(e) => {
            return format!(
                "Chrome at {} did not start; no reason could be found ({e})",
                chrome.display()
            );
        }
    };
    let mut child = match Command::new(chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-first-run",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return format!(
                "Chrome at {} did not start: it could not be run ({e})",
                chrome.display()
            );
        }
    };
    let printed = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let printed = Arc::clone(&printed);
        // Not joined: a Chrome that was killed can leave helpers holding the pipe open.
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut printed = printed.lock().unwrap_or_else(|e| e.into_inner());
                printed.push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        });
    }
    let deadline = Instant::now() + timeout;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) | Err(_) => break None,
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let Some(status) = exited else {
        return format!(
            "Chrome at {} did not start for the server but stayed up when run again by itself; the cause is not in Chrome's own output",
            chrome.display()
        );
    };
    // The reader may be a moment behind the exit.
    std::thread::sleep(Duration::from_millis(100));
    let printed = printed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let lines: Vec<&str> = printed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !CHROME_NOISE.iter().any(|noise| line.contains(noise)))
        .collect();
    if lines.is_empty() {
        return format!(
            "Chrome at {} exited ({status}) when run again and printed nothing to say why",
            chrome.display()
        );
    }
    let said = lines[lines.len().saturating_sub(8)..]
        .iter()
        .map(|line| line.chars().take(300).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n    ");
    format!(
        "Chrome at {} exited ({status}) when run again; it said:\n    {said}",
        chrome.display()
    )
}

impl ChromeRenderDisplayImageGenerator {
    pub fn new(chrome_install_dir: PathBuf, idle_timeout: Duration, source: ChromeSource) -> Self {
        Self {
            inner: Arc::new(Inner {
                chrome_install_dir,
                idle_timeout,
                source,
                browser: Mutex::new(None),
                chrome_pid: AtomicU32::new(0),
            }),
            templates: Templates::built_in(),
        }
    }

    /// Draws with `templates` (the built-in ones are what it has otherwise).
    pub fn with_templates(mut self, templates: Templates) -> Self {
        self.templates = templates;
        self
    }

    /// The page for `layout` as HTML. Fonts are referenced by relative path, so it
    /// renders correctly from a directory holding [`ASSETS`].
    pub fn render_html(
        &self,
        layout: &LayoutName,
        glance_data: &GlanceData,
    ) -> anyhow::Result<String> {
        self.templates.render(layout, glance_data)
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

/// A registry holding the partials every layout can use, and `templates` (a name and its source each) to draw.
pub(super) fn registry<'a>(
    templates: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> anyhow::Result<Handlebars<'static>> {
    let mut handlebars = Handlebars::new();
    for (name, part) in PARTS {
        handlebars.register_partial(name, part)?;
    }
    for (name, svg) in ICONS {
        // Drop the XML prolog and comments: they are invalid inside HTML.
        let svg = &svg[svg.find("<svg").unwrap_or(0)..];
        handlebars.register_partial(name, svg)?;
    }
    for (name, source) in templates {
        // Under a name of their own: partials and templates share a namespace, and a layout may well be called
        // `departures` or `header`.
        handlebars
            .register_template_string(&template_name(name), source)
            .with_context(|| format!("The layout {name} is not valid"))?;
    }
    Ok(handlebars)
}

/// What a layout's template is registered as, apart from the partials.
pub(super) fn template_name(layout: &str) -> String {
    format!("layout/{layout}")
}

/// `template` drawn once with `glance_data`: for trying a template, not for drawing every run.
pub(super) fn render_page(template: &str, glance_data: &GlanceData) -> anyhow::Result<String> {
    Ok(registry([("page", template)])?.render(&template_name("page"), glance_data)?)
}

impl Inner {
    /// Screenshots `page`, reusing the running Chrome where possible.
    ///
    /// `abandoned` is set if the render has been given up on, in which case it stops rather than
    /// launch a Chrome nobody is waiting for.
    fn capture(
        &self,
        page: &Path,
        profile: &DisplayProfile,
        abandoned: &AtomicBool,
    ) -> anyhow::Result<Vec<u8>> {
        let mut slot = self.browser.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(launched) = slot.as_ref() {
            match capture_in_new_tab(&launched.browser, page, profile) {
                Ok(png) => return Ok(png),
                Err(e) => {
                    tracing::warn!("Render with the running Chrome failed ({e:#}); relaunching")
                }
            }
        }

        anyhow::ensure!(
            !abandoned.load(Ordering::SeqCst),
            "The render was given up on"
        );
        // Drop the old one first so its process is gone before the next starts.
        *slot = None;
        self.chrome_pid.store(0, Ordering::SeqCst);
        let browser = self.launch(WINDOW)?;
        self.chrome_pid
            .store(browser.get_process_id().unwrap_or(0), Ordering::SeqCst);
        match capture_in_new_tab(&browser, page, profile) {
            Ok(png) => {
                *slot = Some(LaunchedBrowser { browser });
                Ok(png)
            }
            Err(e) => {
                // The browser is dropped (and stopped) on the way out.
                self.chrome_pid.store(0, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    fn launch(&self, window: (u32, u32)) -> anyhow::Result<Browser> {
        let fetcher_options = FetcherOptions::default()
            .with_allow_download(true)
            .with_install_dir(Some(self.chrome_install_dir.clone()));
        let path = match self.source {
            ChromeSource::PreferSystem => find_system_chrome(),
            ChromeSource::Bundled => {
                tracing::info!(
                    "Using the bundled Chrome in {}",
                    self.chrome_install_dir.display()
                );
                None
            }
        };
        let launcher_options = LaunchOptions::default_builder()
            .window_size(Some(window))
            .devtools(false)
            .headless(true)
            // None makes headless_chrome download its pinned revision instead.
            .path(path.clone())
            .fetcher_options(fetcher_options)
            .idle_browser_timeout(self.idle_timeout)
            .build()?;
        let browser = match Browser::new(launcher_options) {
            Ok(browser) => browser,
            Err(e) => {
                // The crate reads what Chrome says on the way out and drops it; the reason is in that.
                let chrome = path.or_else(|| find_downloaded_chrome(&self.chrome_install_dir));
                if let Some(chrome) = chrome {
                    tracing::warn!("{}", explain_launch_failure(&chrome, EXPLAIN_TIMEOUT));
                }
                return Err(e).context("Failed to create browser instance");
            }
        };
        match browser.get_version() {
            Ok(v) => tracing::info!(
                "Rendering with {} (protocol {})",
                v.product,
                v.protocol_version
            ),
            Err(e) => tracing::warn!("Could not read Chrome version: {e}"),
        }
        browser.set_default_timeout(RENDER_TIMEOUT);
        Ok(browser)
    }
}

/// Renders in a tab of its own and always closes it, so a kept Chrome doesn't pile them up.
fn capture_in_new_tab(
    browser: &Browser,
    page: &Path,
    profile: &DisplayProfile,
) -> anyhow::Result<Vec<u8>> {
    let tab = browser.new_tab().context("Failed to create new tab")?;
    let result = capture_in_tab(&tab, page, profile);
    // Not close(false): the pinned Chromium never answers Target.closeTarget, so that call
    // waits out its timeout and leaves the connection dead, defeating reuse.
    if let Err(e) = tab.close(true) {
        tracing::warn!("Could not close the render tab: {e}");
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
        layout: &LayoutName,
    ) -> anyhow::Result<ImageData> {
        let html = self.render_html(layout, &data)?;
        // Keep the directory alive until the screenshot has been taken.
        let dir = tempfile::tempdir().context("Failed to create render directory")?;
        let page = self.write_page(dir.path(), &html)?;
        // headless_chrome blocks, so keep it off the async runtime's threads.
        let inner = Arc::clone(&self.inner);
        let profile = *profile;
        let abandoned = Arc::new(AtomicBool::new(false));
        let mut guard = AbandonGuard::new(Arc::clone(&inner), Arc::clone(&abandoned));
        let rendered =
            tokio::task::spawn_blocking(move || inner.capture(&page, &profile, &abandoned)).await;
        guard.disarm();
        let png = rendered.context("Render task panicked")??;
        drop(dir);
        Ok(ImageData::new(png))
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::models::DateInfo;
    use crate::domain::models::display::Palette;
    use crate::domain::models::weather::{WeatherCondition, WeatherInformation};
    use chrono_tz::Europe::London;

    use super::*;

    fn html_for(data: &GlanceData) -> String {
        let generator = ChromeRenderDisplayImageGenerator::new(
            PathBuf::from("unused"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        );
        generator
            .render_html(&LayoutName::parse(DASHBOARD).unwrap(), data)
            .unwrap()
    }

    #[test]
    fn a_fresh_dashboard_carries_no_staleness_markers() {
        let html = html_for(&GlanceData::sample(
            chrono::Utc::now().with_timezone(&London),
        ));
        assert!(!html.contains(" ago<"), "no age label expected");
        assert!(!html.contains("Unavailable"));
        assert!(!html.contains("Weather unavailable"));
        assert!(html.contains("container has-weather"));
    }

    #[test]
    fn degraded_data_shows_ages_and_unavailable_parts() {
        let html = html_for(&GlanceData::sample_degraded(
            chrono::Utc::now().with_timezone(&London),
        ));
        assert!(html.contains("8 min ago</span>"));
        assert!(html.contains("Updated 40 min ago"));
        assert!(html.contains(">Unavailable: service error</div>"));
    }

    #[test]
    fn unavailable_weather_keeps_its_column_and_says_so() {
        let data = GlanceData::new(
            None,
            vec![],
            DateInfo::new(chrono::Utc::now().with_timezone(&London)),
        )
        .with_weather_unavailable("timed out");
        let html = html_for(&data);
        assert!(html.contains("Weather unavailable: timed out"));
        assert!(html.contains("container has-weather"));

        // Weather that is simply switched off leaves no column and no message.
        let off = html_for(&GlanceData::new(
            None,
            vec![],
            DateInfo::new(chrono::Utc::now().with_timezone(&London)),
        ));
        assert!(!off.contains("Weather unavailable"));
        assert!(!off.contains("container has-weather"));
    }

    fn page_of(layout: &str, data: &GlanceData) -> String {
        ChromeRenderDisplayImageGenerator::new(
            PathBuf::from("unused"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        )
        .render_html(&LayoutName::parse(layout).unwrap(), data)
        .unwrap()
    }

    #[test]
    fn the_dashboard_is_the_default_layout_and_every_layout_renders_its_own_page() {
        let layouts = Templates::built_in().layouts();
        assert_eq!(layouts.default_layout().name.as_str(), DASHBOARD);
        let data = GlanceData::sample(chrono::Utc::now().with_timezone(&London));

        for layout in layouts.all() {
            let html = page_of(layout.name.as_str(), &data);
            // Every partial a page names was found and filled in, and the data reached the page.
            assert!(!html.contains("{{"), "{}", layout.name);
            assert!(html.contains("<style>"), "{}", layout.name);
            assert!(html.contains("class=\"header\""), "{}", layout.name);
            assert!(html.contains("class=\"departures\""), "{}", layout.name);
        }
    }

    #[test]
    fn the_departures_layout_leaves_the_weather_out() {
        let data = GlanceData::sample(chrono::Utc::now().with_timezone(&London));

        let dashboard = page_of(DASHBOARD, &data);
        let departures = page_of("departures", &data);

        assert!(dashboard.contains("class=\"weather\""));
        assert!(dashboard.contains("container has-weather"));
        assert!(!departures.contains("class=\"weather\""));
        assert!(!departures.contains("has-weather\">"));
        // The same boards, though.
        assert!(departures.contains("class=\"board\""));
    }

    #[test]
    fn a_layout_that_does_not_exist_is_an_error() {
        let generator = ChromeRenderDisplayImageGenerator::new(
            PathBuf::from("unused"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        );
        let data = GlanceData::sample(chrono::Utc::now().with_timezone(&London));

        let error = generator
            .render_html(&LayoutName::parse("nonesuch").unwrap(), &data)
            .unwrap_err();

        assert!(format!("{error}").contains("nonesuch"), "{error}");
    }

    /// A program that prints `words` and exits, on any platform.
    fn printing(words: &str) -> Command {
        let mut command;
        if cfg!(windows) {
            command = Command::new("cmd");
            command.args(["/C", "echo", words]);
        } else {
            command = Command::new("echo");
            command.arg(words);
        }
        command
    }

    /// A program that does nothing for half a minute, on any platform.
    fn sleeping() -> Command {
        let mut command;
        if cfg!(windows) {
            command = Command::new("cmd");
            command.args(["/C", "ping", "-n", "30", "127.0.0.1", ">", "nul"]);
        } else {
            command = Command::new("sleep");
            command.arg("30");
        }
        command
    }

    #[test]
    fn what_a_program_prints_is_given_without_the_line_break() {
        let printed = output_within(printing("Chrome-1.2.3"), Duration::from_secs(20)).unwrap();

        assert_eq!(printed, "Chrome-1.2.3");
    }

    #[test]
    fn a_program_that_does_not_finish_is_given_up_on_and_stopped_and_does_not_hold_the_render() {
        let began = Instant::now();

        let result = output_within(sleeping(), Duration::from_millis(300));

        let reason = result.unwrap_err();
        assert!(reason.contains("did not finish"), "{reason}");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "waited {:?} for a program that was to be given up on after 0.3 s",
            began.elapsed()
        );
    }

    /// A Chrome that does not start: it complains and exits.
    #[cfg(unix)]
    fn failing_chrome(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let chrome = dir.join("chrome");
        std::fs::write(
            &chrome,
            "#!/bin/sh\necho 'dbus: no session bus' >&2\necho 'FATAL:sandbox_linux.cc: No usable sandbox!' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&chrome, std::fs::Permissions::from_mode(0o755)).unwrap();
        chrome
    }

    #[cfg(unix)]
    #[test]
    fn a_chrome_that_will_not_start_has_its_reason_given_without_the_noise() {
        let dir = tempfile::tempdir().unwrap();
        let chrome = failing_chrome(dir.path());

        let said = explain_launch_failure(&chrome, Duration::from_secs(20));

        assert!(said.contains("No usable sandbox"), "{said}");
        assert!(said.contains("exited"), "{said}");
        assert!(!said.contains("dbus"), "{said}");
    }

    #[test]
    fn a_chrome_that_cannot_be_run_is_said_to_be_so() {
        let said = explain_launch_failure(Path::new("/nowhere/chrome"), Duration::from_secs(20));

        assert!(said.contains("could not be run"), "{said}");
    }

    #[test]
    fn a_downloaded_chrome_is_found_in_its_folders() {
        let name = if cfg!(windows) {
            "chrome.exe"
        } else {
            "chrome"
        };
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("linux-1095492").join("chrome-linux");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("chrome_sandbox"), "").unwrap();
        std::fs::write(folder.join(name), "").unwrap();

        assert_eq!(find_downloaded_chrome(dir.path()), Some(folder.join(name)));
        assert_eq!(find_downloaded_chrome(&dir.path().join("none")), None);
    }

    #[test]
    fn a_program_that_is_not_there_is_an_error_and_not_a_wait() {
        let result = output_within(
            Command::new("a-program-that-does-not-exist"),
            Duration::from_secs(20),
        );

        assert!(result.unwrap_err().contains("could not be run"));
    }

    #[test]
    fn file_urls_encode_characters_that_need_it() {
        #[cfg(unix)]
        let (path, expected) = (
            "/tmp/my render/dashboard é.html",
            "file:///tmp/my%20render/dashboard%20%C3%A9.html",
        );
        #[cfg(windows)]
        let (path, expected) = (
            r"C:\tmp\my render\dashboard é.html",
            "file:///C:/tmp/my%20render/dashboard%20%C3%A9.html",
        );

        let url = file_url(Path::new(path)).unwrap();

        assert_eq!(url.as_str(), expected);
    }

    #[test]
    fn file_urls_need_an_absolute_path() {
        assert!(file_url(Path::new("dashboard.html")).is_err());
    }

    #[cfg(unix)]
    fn a_stand_in_for_chrome() -> (Arc<Inner>, std::process::Child) {
        let generator = ChromeRenderDisplayImageGenerator::new(
            PathBuf::from("unused"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        );
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        generator
            .inner
            .chrome_pid
            .store(child.id(), Ordering::SeqCst);
        (generator.inner, child)
    }

    #[cfg(unix)]
    #[test]
    fn a_render_given_up_on_stops_its_chrome_and_says_so() {
        let (inner, mut child) = a_stand_in_for_chrome();
        let abandoned = Arc::new(AtomicBool::new(false));

        drop(AbandonGuard::new(inner, Arc::clone(&abandoned)));

        let status = child.wait().unwrap();
        assert!(!status.success(), "{status:?}");
        assert!(abandoned.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[test]
    fn a_render_that_ended_leaves_chrome_running() {
        let (inner, mut child) = a_stand_in_for_chrome();
        let abandoned = Arc::new(AtomicBool::new(false));

        let mut guard = AbandonGuard::new(inner, Arc::clone(&abandoned));
        guard.disarm();
        drop(guard);

        assert!(
            child.try_wait().unwrap().is_none(),
            "Chrome should still be running"
        );
        assert!(!abandoned.load(Ordering::SeqCst));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn an_abandoned_render_does_not_launch_a_chrome_for_nobody() {
        let generator = ChromeRenderDisplayImageGenerator::new(
            PathBuf::from("unused"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        );
        let profile = DisplayProfile {
            width: 10,
            height: 10,
            palette: Palette::Mono,
        };

        let error = generator
            .inner
            .capture(
                Path::new("/nonexistent.html"),
                &profile,
                &AtomicBool::new(true),
            )
            .unwrap_err();

        assert!(format!("{error}").contains("given up on"), "{error}");
    }

    /// Needs a real Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn renders_a_png_at_the_profile_size() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile {
            width: 800,
            height: 480,
            palette: Palette::Mono,
        };
        let data = GlanceData::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![],
            DateInfo::new(chrono::Utc::now().with_timezone(&London)),
        );

        let image = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("home_display_test_chrome"),
            DEFAULT_IDLE_TIMEOUT,
            ChromeSource::PreferSystem,
        )
        .generate(data, &profile, &dashboard())
        .await
        .unwrap();

        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
    }

    fn dashboard() -> LayoutName {
        LayoutName::parse(DASHBOARD).unwrap()
    }

    fn sample_data() -> GlanceData {
        GlanceData::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![],
            DateInfo::new(chrono::Utc::now().with_timezone(&London)),
        )
    }

    /// The process id of the Chrome being kept, if any.
    fn kept_pid(generator: &ChromeRenderDisplayImageGenerator) -> Option<u32> {
        let slot = generator.inner.browser.lock().unwrap();
        slot.as_ref().and_then(|l| l.browser.get_process_id())
    }

    /// Needs a real Chrome, so run with `--ignored`. Several outputs are drawn for in one run, each at its own size, and the
    /// one Chrome serves them all: neither does a size left over from the last picture limit the next, nor does a change of
    /// size cost a relaunch.
    #[tokio::test]
    #[ignore]
    async fn one_chrome_draws_pictures_of_different_sizes_each_at_its_own() {
        let generator = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("home_display_test_chrome"),
            Duration::from_secs(300),
            ChromeSource::PreferSystem,
        );
        let sizes = [
            (800, 480),
            (1920, 1080),
            (1872, 1404),
            (800, 480),
            (3840, 2160),
        ];
        let mut pid = None;

        for (width, height) in sizes {
            let profile = DisplayProfile {
                width,
                height,
                palette: Palette::Rgb,
            };
            let image = generator
                .generate(
                    // For looking at, the whole sample (boards and weather), and otherwise a little.
                    if std::env::var("RENDER_SHOTS").is_ok() {
                        GlanceData::sample(chrono::Utc::now().with_timezone(&London))
                    } else {
                        sample_data()
                    },
                    &profile,
                    &dashboard(),
                )
                .await
                .unwrap();
            let decoded = image::load_from_memory(&image.data).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (width, height));
            // For looking at: where RENDER_SHOTS names a folder, each picture as it came out.
            if let Ok(folder) = std::env::var("RENDER_SHOTS") {
                std::fs::write(format!("{folder}/{width}x{height}.png"), &image.data).unwrap();
            }
            let now = kept_pid(&generator).expect("Chrome is kept");
            assert_eq!(
                *pid.get_or_insert(now),
                now,
                "{width}x{height} relaunched Chrome"
            );
        }
    }

    /// Needs a real Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn keeps_one_chrome_across_renders_and_relaunches_when_it_dies() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile {
            width: 800,
            height: 480,
            palette: Palette::Mono,
        };
        let generator = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("home_display_test_chrome"),
            Duration::from_secs(300),
            ChromeSource::PreferSystem,
        );

        generator
            .generate(sample_data(), &profile, &dashboard())
            .await
            .unwrap();
        let first = kept_pid(&generator).expect("Chrome is kept after a render");
        generator
            .generate(sample_data(), &profile, &dashboard())
            .await
            .unwrap();
        assert_eq!(kept_pid(&generator), Some(first), "second render reused it");

        assert!(
            Command::new("kill")
                .arg("-9")
                .arg(first.to_string())
                .status()
                .unwrap()
                .success()
        );
        std::thread::sleep(Duration::from_millis(500));
        generator
            .generate(sample_data(), &profile, &dashboard())
            .await
            .unwrap();
        let relaunched = kept_pid(&generator).expect("Chrome is kept after relaunch");
        assert_ne!(relaunched, first, "a dead Chrome was replaced");
    }

    /// Needs network on first run to download Chrome, so run with `--ignored`.
    #[tokio::test]
    #[ignore]
    async fn renders_and_reuses_the_bundled_chrome() {
        let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
        let profile = DisplayProfile {
            width: 800,
            height: 480,
            palette: Palette::Mono,
        };
        let generator = ChromeRenderDisplayImageGenerator::new(
            std::env::temp_dir().join("home_display_test_chrome"),
            Duration::from_secs(300),
            ChromeSource::Bundled,
        );

        let started = std::time::Instant::now();
        let image = generator
            .generate(sample_data(), &profile, &dashboard())
            .await
            .unwrap();
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (800, 480));
        let first = kept_pid(&generator).expect("Chrome is kept after a render");

        let second_started = std::time::Instant::now();
        generator
            .generate(sample_data(), &profile, &dashboard())
            .await
            .unwrap();
        assert_eq!(kept_pid(&generator), Some(first), "second render reused it");
        assert!(
            second_started.elapsed() < Duration::from_secs(15),
            "a reused render took {:?} (first took {:?})",
            second_started.elapsed(),
            started.elapsed()
        );
    }
}

use anyhow::Error;
use headless_chrome::{Browser, FetcherOptions, LaunchOptions};
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use anyhow::{Context, Result};
pub async fn capture_webpage(html: String) -> Result<Vec<u8>> {

    let fetcher_options = FetcherOptions::default().with_allow_download(true);
    let launcher_options = LaunchOptions::default_builder()
        .window_size(Some((800,480)))
        .devtools(false)
        .headless(true)
        .fetcher_options(fetcher_options)
        .build()?;
    let browser = Browser::new(launcher_options)
        .context("Failed to create browser instance")?;
    let tab = browser.new_tab()
        .context("Failed to create new tab")?;
    tab.navigate_to(format!("data:text/html;charset=utf-8,{}", html).as_str())?.wait_until_navigated()?;


    let screenshot_options = CaptureScreenshotFormatOption::Png;
    let png_data = tab.capture_screenshot(screenshot_options, None, None, false)
        .context("Failed to capture screenshot")?;

    Ok(png_data)
}


pub fn save_screenshot(screenshot_data: &[u8], output_path: &str) -> Result<(), Error> {
    std::fs::write(output_path, screenshot_data)?;
    Ok(())
}



use anyhow::{Context, Result};
use clap::Parser;
use image::GrayImage;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::adapters::departures::setup_departure_boards;
use eink_home_display_rust::adapters::display_image_generator::chrome_render::ChromeRenderDisplayImageGenerator;
use eink_home_display_rust::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use eink_home_display_rust::adapters::image_display_service::quantise::quantise_grey;
use eink_home_display_rust::adapters::image_display_service::setup_display;
use eink_home_display_rust::adapters::weather::setup_weather_service;
use eink_home_display_rust::application::Application;
use eink_home_display_rust::cli::RenderArgs;
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::domain::models::display::{Dither, DisplayProfile, Palette};
use eink_home_display_rust::domain::models::image::ImageData;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::models::GlanceData;
use eink_home_display_rust::domain::services::display_image_generator::DisplayImageGenerator;
use eink_home_display_rust::domain::services::image_display_service::ImageDisplayService;
use eink_home_display_rust::domain::services::image_repository::ImageRepository;

/// Renders the dashboard to PNG files without touching the e-paper panel.
#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let args = RenderArgs::parse();

    let config = args
        .config_file
        .as_ref()
        .map(|path| ApplicationConfig::new(path).with_context(|| format!("Failed to load {}", path.display())))
        .transpose()?;
    if args.live && config.is_none() {
        anyhow::bail!("--live needs --config-file");
    }

    let profile = match &config {
        Some(config) => setup_display(&config.display).profile(),
        None => EinkWaveshareAdapter::new(Dither::None).profile(),
    };
    let dither = args
        .dither
        .map(Dither::from)
        .or(config.as_ref().map(|c| c.display.dither))
        .unwrap_or_default();

    let generator = ChromeRenderDisplayImageGenerator::new();
    let data = match (&config, args.live) {
        (Some(config), true) => fetch_live(config).await?,
        _ => GlanceData::sample(chrono::Local::now()),
    };

    if let Some(dir) = &args.html_dir {
        std::fs::create_dir_all(dir)?;
        let page = generator.write_page(dir, &generator.render_html(&data)?)?;
        log::info!("Wrote {}", page.display());
    }

    let image = generator.generate(data, &profile).await?;
    ImageOutput(args.output.clone()).store(&image).await?;
    log::info!("Wrote {}", args.output.display());

    let dithered_path = args.output.with_extension("dithered.png");
    preview_on_panel(&image, &profile, dither)?.save(&dithered_path)?;
    log::info!("Wrote {} (dither: {dither:?})", dithered_path.display());
    Ok(())
}

async fn fetch_live(config: &ApplicationConfig) -> Result<GlanceData> {
    // Reuse the application's own fetching by running it against a capture-only generator.
    let (tx, rx) = std::sync::mpsc::channel();
    let app = Application::new(
        setup_weather_service(&config.weather),
        Capture(tx),
        NoDisplay,
        NoStore,
        setup_departure_boards(&config.departures, &config.providers)?,
    );
    app.run(Location::new(config.location.latitude, config.location.longitude))
        .await?;
    Ok(rx.recv()?)
}

/// What the panel would show: greyscale reduced to the profile's levels.
fn preview_on_panel(image: &ImageData, profile: &DisplayProfile, dither: Dither) -> Result<GrayImage> {
    let levels = match profile.palette {
        Palette::Mono => 2,
        Palette::Grey(levels) => levels,
        Palette::Indexed(_) | Palette::Rgb => 255,
    };
    let luma = image::load_from_memory(&image.data)?.to_luma8();
    Ok(quantise_grey(&luma, levels, dither))
}

struct ImageOutput(std::path::PathBuf);

#[async_trait::async_trait]
impl ImageRepository for ImageOutput {
    async fn store(&self, image: &ImageData) -> Result<()> {
        std::fs::write(&self.0, &image.data).with_context(|| format!("Failed to write {}", self.0.display()))
    }
}

/// Hands the fetched data back to `fetch_live` instead of rendering it.
struct Capture(std::sync::mpsc::Sender<GlanceData>);

impl DisplayImageGenerator for Capture {
    async fn generate(&self, data: GlanceData, _: &DisplayProfile) -> Result<ImageData> {
        self.0.send(data)?;
        Ok(ImageData::new(vec![]))
    }
}

struct NoStore;

#[async_trait::async_trait]
impl ImageRepository for NoStore {
    async fn store(&self, _: &ImageData) -> Result<()> {
        Ok(())
    }
}

struct NoDisplay;

#[async_trait::async_trait]
impl ImageDisplayService for NoDisplay {
    fn profile(&self) -> DisplayProfile {
        DisplayProfile { width: 0, height: 0, palette: Palette::Mono }
    }

    async fn display(&self, _: &ImageData) -> Result<()> {
        Ok(())
    }
}

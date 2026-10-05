use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use image::GrayImage;

use eink_home_display_rust::adapters::display_image_generator::chrome_render::{ChromeRenderDisplayImageGenerator, ChromeSource, DEFAULT_IDLE_TIMEOUT};
use eink_home_display_rust::adapters::image_display_service::quantise::quantise_grey;
use eink_home_display_rust::adapters::clock::SystemClock;
use eink_home_display_rust::bootstrap::{self, setup_display};
use eink_home_display_rust::adapters::published_images::DirectoryImages;
use eink_home_display_rust::cli::RenderArgs;
use eink_home_display_rust::config::cache::{CacheConfig, CachePaths};
use eink_home_display_rust::config::application::{ApplicationConfig, DisplayConfig, DisplayKind};
use eink_home_display_rust::domain::models::display::{Dither, DisplayProfile, Palette};
use eink_home_display_rust::domain::models::image::ImageData;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::models::GlanceData;
use eink_home_display_rust::domain::services::display_image_generator::DisplayImageGenerator;
use eink_home_display_rust::domain::services::image_display_service::ImageDisplayService;
use eink_home_display_rust::domain::services::image_repository::ImageRepository;

/// Renders the dashboard to PNG files without touching any display, one set per display type.
#[tokio::main]
async fn main() -> Result<()> {
    bootstrap::init_logging("info");
    let args = RenderArgs::parse();

    let config = args
        .config_file
        .as_ref()
        .map(|path| bootstrap::load_application_config(path))
        .transpose()?;
    if args.live && config.is_none() {
        anyhow::bail!("--live needs --config-file");
    }

    let kinds: Vec<DisplayKind> = if args.display.is_empty() {
        DisplayKind::ALL.to_vec()
    } else {
        args.display.iter().copied().map(DisplayKind::from).collect()
    };
    let dither = args
        .dither
        .map(Dither::from)
        .or(config.as_ref().map(|c| c.display.dither.into()))
        .unwrap_or_default();

    let data = match (&config, args.live) {
        (Some(config), true) => fetch_live(config).await?,
        _ if args.degraded => GlanceData::sample_degraded(chrono::Local::now()),
        _ => GlanceData::sample(chrono::Local::now()),
    };

    std::fs::create_dir_all(&args.output_dir)
        .with_context(|| format!("Failed to create {}", args.output_dir.display()))?;
    let cache = CachePaths::new(
        args.cache_dir
            .clone()
            .or_else(|| config.as_ref().map(|c| c.cache.directory.clone()))
            .unwrap_or_else(|| CacheConfig::default().directory),
    );
    cache.ensure_exists()?;
    let generator = ChromeRenderDisplayImageGenerator::new(
        cache.chrome(),
        DEFAULT_IDLE_TIMEOUT,
        ChromeSource::from(args.bundled_chrome),
    );

    for kind in kinds {
        let name = file_name(kind);
        let profile = setup_display(&DisplayConfig { kind, dither: dither.into(), image_format: Default::default() }, Arc::new(DirectoryImages::new(""))).profile();
        log::info!("Rendering for {name} ({}x{}, {:?})", profile.width, profile.height, profile.palette);

        if args.html {
            let dir = args.output_dir.join(format!("page_{name}"));
            std::fs::create_dir_all(&dir)?;
            generator.write_page(&dir, &generator.render_html(&data)?)?;
        }

        let image = generator.generate(data.clone(), &profile).await?;
        let render_path = args.output_dir.join(format!("page_render_{name}.png"));
        let output_path = args.output_dir.join(format!("output_{name}.png"));
        std::fs::write(&render_path, &image.data)
            .with_context(|| format!("Failed to write {}", render_path.display()))?;
        preview_on_panel(&image, &profile, dither)?.save(&output_path)?;
        log::info!("Wrote {} and {} (dither: {dither:?})", render_path.display(), output_path.display());
    }
    Ok(())
}

fn file_name(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::WaveshareEpd7in5V2 => "Waveshare_EPD7in5V2",
        DisplayKind::ReTerminalE1003 => "reTerminal_E1003",
    }
}

async fn fetch_live(config: &ApplicationConfig) -> Result<GlanceData> {
    // Reuse the application's own fetching by running it against a capture-only generator.
    let (tx, rx) = std::sync::mpsc::channel();
    let app = bootstrap::assemble(config, Capture(tx), NoDisplay, NoStore, Arc::new(SystemClock))?;
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

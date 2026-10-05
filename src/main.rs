use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use serde_valid::Validate;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::adapters::departures::setup_departure_boards;
use eink_home_display_rust::adapters::display_image_generator::chrome_render::{ChromeRenderDisplayImageGenerator, ChromeSource, DEFAULT_IDLE_TIMEOUT};
use eink_home_display_rust::adapters::image_display_service::setup_display;
use eink_home_display_rust::adapters::image_repository::file_store::FileStoreImageRepository;
use eink_home_display_rust::adapters::image_server::{serve, RefreshControl};
use eink_home_display_rust::adapters::weather::setup_weather_service;
use eink_home_display_rust::application::Application;
use eink_home_display_rust::cli;
use eink_home_display_rust::launch;
use eink_home_display_rust::scheduler::{run_periodically_from, shutdown_signal, PERIODIC_IDLE_TIMEOUT};
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::config::cache::CachePaths;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::services::departures_service::DeparturesService;
use eink_home_display_rust::domain::services::display_image_generator::DisplayImageGenerator;
use eink_home_display_rust::domain::services::image_repository::ImageRepository;
use eink_home_display_rust::domain::services::weather_service::WeatherService;
use eink_home_display_rust::domain::services::ImageDisplayService;

#[tokio::main]
async fn main() -> Result<()> {
    initialize_logging();

    let args = match cli::Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            log::error!("Failed to parse command-line arguments: {}", e);
            eprintln!("Error: Invalid command-line arguments. Use --help for usage information.");
            std::process::exit(1);
        }
    };

    let config = match ApplicationConfig::new(&args.config_file) {
        Ok(config) => config,
        Err(e) => {
            log::error!("Failed to load settings: {}", e);
            eprintln!(
                "Error: Failed to load configuration from {}. Please check your config file.",
                args.config_file.display()
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = config.validate() {
        log::error!("Configuration validation failed: {}", e);
        eprintln!("Error: Configuration is invalid. Please check your config file.");
        std::process::exit(1);
    }

    log::info!("Settings loaded successfully: {:?}", config);

    let location = Location::new(config.location.latitude, config.location.longitude);
    let cache = CachePaths::new(args.cache_dir.clone().unwrap_or_else(|| config.cache.directory.clone()));
    cache.ensure_exists()?;
    let chrome_source = ChromeSource::from(args.bundled_chrome);

    let Some(schedule) = args.schedule() else {
        return create_application(&config, &cache, DEFAULT_IDLE_TIMEOUT, chrome_source)?.run(location).await;
    };
    // Built once so the Chrome it launches is kept between runs.
    let app = create_application(&config, &cache, PERIODIC_IDLE_TIMEOUT, chrome_source)?;
    let now = chrono::Local::now();
    let marker = cache.render_attempt();
    let image_is_current = config.display.kind.fetches_image()
        && launch::served_image_is_current(&config.server.directory, config.display.image_format, schedule, now);
    let first = launch::first_render_at(
        now,
        args.no_initial_run,
        image_is_current,
        launch::last_attempt(&marker),
        args.restart_cooldown,
    );
    match first {
        Some(at) if at > now => log::info!("Holding the first render until {} (restart cooldown)", at.format("%H:%M:%S")),
        Some(_) => log::info!("Rendering now"),
        None if image_is_current => log::info!("The served image is current; waiting for the next slot"),
        None => {}
    }
    let refresh = RefreshControl::new(Duration::from_secs(config.server.refresh_cooldown_seconds.into()));
    let periodic = run_periodically_from(schedule, first, refresh.wake(), shutdown_signal(), || {
        launch::record_attempt(&marker);
        refresh.render_started();
        let run = app.run(location);
        let refresh = &refresh;
        async move {
            let result = run.await;
            refresh.render_finished();
            result
        }
    });
    if !config.display.kind.fetches_image() {
        return periodic.await;
    }
    // The display downloads its image, so serve it for as long as the refresh loop runs.
    tokio::select! {
        result = periodic => result,
        result = serve(&config.server, config.display.image_format, schedule.clone(), refresh.clone()) => result,
    }
}

fn initialize_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));

    fmt().with_env_filter(env_filter).init();
    // TODO: Figure out how to log the current log level as RUST_LOG should by able to override via try_from_default_env
    let current_level = tracing::level_filters::STATIC_MAX_LEVEL;
    tracing::info!("Logging initialized at {} level", current_level);
}

fn create_application(
    config: &ApplicationConfig,
    cache: &CachePaths,
    chrome_idle_timeout: Duration,
    chrome_source: ChromeSource,
) -> Result<
    Application<
        impl WeatherService,
        impl DisplayImageGenerator,
        impl ImageDisplayService,
        impl ImageRepository,
        impl DeparturesService,
    >,
> {
    Ok(Application::new(
        setup_weather_service(&config.weather)?,
        ChromeRenderDisplayImageGenerator::new(cache.chrome(), chrome_idle_timeout, chrome_source),
        setup_display(&config.display, &config.server.directory),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
        setup_departure_boards(&config.departures, &config.providers)?,
    ))
}

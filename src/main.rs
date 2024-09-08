use anyhow::Result;
use clap::Parser;
use serde_valid::Validate;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::adapters::display_image_generator::chrome_render::ChromeRenderDisplayImageGenerator;
use eink_home_display_rust::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use eink_home_display_rust::adapters::image_repository::file_store::FileStoreImageRepository;
use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use eink_home_display_rust::application::Application;
use eink_home_display_rust::cli;
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::config::weather::{WeatherConfig, WeatherProvider};
use eink_home_display_rust::domain::models::location::Location;
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
    create_application(&config).run(location).await
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
) -> Application<
    impl WeatherService,
    impl DisplayImageGenerator,
    impl ImageDisplayService,
    impl ImageRepository,
> {
    Application::new(
        setup_weather_service(&config.weather),
        ChromeRenderDisplayImageGenerator::new(config.image.width, config.image.height),
        EinkWaveshareAdapter::new(),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
    )
}

fn setup_weather_service(config: &WeatherConfig) -> impl WeatherService {
    match config.provider {
        WeatherProvider::OpenWeather => OpenWeatherWeatherServiceAdapter::new(
            config.open_weather.host_url.clone(),
            config.open_weather.api_key.clone(),
            reqwest::Client::new(),
        ),
    }
}

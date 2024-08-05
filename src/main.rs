use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use serde_valid::Validate;
use tracing_subscriber::{EnvFilter, fmt};

use eink_home_display_rust::adapters::display_image_generator::chrome_render::ChromeRenderDisplayImageGenerator;
use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use eink_home_display_rust::application::Application;
use eink_home_display_rust::cli;
use eink_home_display_rust::config::application::ApplicationConfig;
use eink_home_display_rust::config::weather::{WeatherConfig, WeatherProvider};
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::services::display_image_generator::DisplayImageGenerator;
use eink_home_display_rust::domain::services::weather_service::WeatherService;

#[tokio::main]
async fn main() -> Result<()> {
    initialize_logging();

    let args = cli::Args::try_parse()?;

    let config =
        ApplicationConfig::new(args.config_file.as_path()).context("Failed to load settings")?;
    config.validate()?;
    log::info!("Settings loaded successfully: {:?}", config);

    let application = create_application(&config);

    let location = Location::new(config.location.latitude, config.location.longitude);
    let result = application.run(location).await;
    log::info!("Image saved to {}", result?.to_string_lossy());

    Ok(())
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
) -> Application<impl WeatherService, impl DisplayImageGenerator> {
    Application::new(
        setup_weather_service(&config.weather),
        ChromeRenderDisplayImageGenerator::new(
            config.output.save_directory.clone(),
            config.image.width,
            config.image.height,
        ),
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

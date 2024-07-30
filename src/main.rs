use anyhow::Context;
use anyhow::Result;
use eink_home_display_rust::adapters::display_image_generator::chrome_render::ChromeRenderDisplayImageGenerator;
use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::{
    OpenWeatherConfig, OpenWeatherWeatherServiceAdapter,
};
use eink_home_display_rust::application::Application;
use eink_home_display_rust::domain::services::display_image_generator::DisplayImageGenerator;
use eink_home_display_rust::domain::services::weather_service::WeatherService;
use eink_home_display_rust::settings::{Settings, WeatherProvider};
use std::path::PathBuf;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    initialize_logging();

    let settings = Settings::new().context("Failed to load settings")?;
    log::info!("Settings loaded successfully: {:?}", settings);

    let application = create_application(&settings);

    let result = application.run().await;
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
    settings: &Settings,
) -> Application<impl WeatherService, impl DisplayImageGenerator> {
    Application::new(
        setup_weather_service(settings),
        ChromeRenderDisplayImageGenerator::new(PathBuf::from("./"), 800, 480),
    )
}

fn setup_weather_service(settings: &Settings) -> impl WeatherService {
    match settings.weather.provider {
        WeatherProvider::OpenWeather => {
            let open_weather_config = OpenWeatherConfig {
                api_key: settings.weather.open_weather.api_key.clone(),
                latitude: settings.weather.open_weather.latitude.clone(),
                longitude: settings.weather.open_weather.longitude.clone(),
            };
            OpenWeatherWeatherServiceAdapter::new(open_weather_config)
        }
    }
}

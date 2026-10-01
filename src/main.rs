use anyhow::Result;
use clap::Parser;
use serde_valid::Validate;
use tracing_subscriber::{fmt, EnvFilter};

use eink_home_display_rust::adapters::departures::setup_departure_boards;
use eink_home_display_rust::adapters::display_image_generator::chrome_render::ChromeRenderDisplayImageGenerator;
use eink_home_display_rust::adapters::image_display_service::eink_waveshare::EinkWaveshareAdapter;
use eink_home_display_rust::adapters::image_repository::file_store::FileStoreImageRepository;
use eink_home_display_rust::adapters::weather::no_op::no_op_weather_service::NoOpWeatherServiceAdapter;
use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use eink_home_display_rust::application::Application;
use eink_home_display_rust::cli;
use eink_home_display_rust::config::application::{ApplicationConfig, DisplayConfig, DisplayKind};
use eink_home_display_rust::config::weather::{WeatherConfig, WeatherProvider};
use eink_home_display_rust::domain::models::display::DisplayProfile;
use eink_home_display_rust::domain::models::image::ImageData;
use eink_home_display_rust::domain::models::location::Location;
use eink_home_display_rust::domain::models::weather::WeatherInformation;
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
    create_application(&config)?.run(location).await
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
        setup_weather_service(&config.weather),
        ChromeRenderDisplayImageGenerator::new(),
        setup_display(&config.display),
        FileStoreImageRepository::new(config.file_store.save_directory.clone()),
        setup_departure_boards(&config.departures, &config.providers)?,
    ))
}

enum WeatherServiceImpl {
    OpenWeather(OpenWeatherWeatherServiceAdapter),
    NoOp(NoOpWeatherServiceAdapter),
}

impl WeatherService for WeatherServiceImpl {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<WeatherInformation> {
        match self {
            WeatherServiceImpl::OpenWeather(service) => {
                service.get_weather_for_location(location).await
            }
            WeatherServiceImpl::NoOp(service) => service.get_weather_for_location(location).await,
        }
    }
}

fn setup_weather_service(config: &WeatherConfig) -> WeatherServiceImpl {
    if !config.enabled {
        return WeatherServiceImpl::NoOp(NoOpWeatherServiceAdapter::new());
    }
    match config.provider {
        WeatherProvider::OpenWeather => WeatherServiceImpl::OpenWeather(
            OpenWeatherWeatherServiceAdapter::new(
                config.open_weather.host_url.clone(),
                config.open_weather.api_key.clone(),
                reqwest::Client::new(),
            ),
        ),
    }
}

enum DisplayImpl {
    WaveshareEpd7in5V2(EinkWaveshareAdapter),
}

#[async_trait::async_trait]
impl ImageDisplayService for DisplayImpl {
    fn profile(&self) -> DisplayProfile {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.profile(),
        }
    }

    async fn display(&self, data: &ImageData) -> anyhow::Result<()> {
        match self {
            DisplayImpl::WaveshareEpd7in5V2(display) => display.display(data).await,
        }
    }
}

fn setup_display(config: &DisplayConfig) -> DisplayImpl {
    match config.kind {
        DisplayKind::WaveshareEpd7in5V2 => {
            DisplayImpl::WaveshareEpd7in5V2(EinkWaveshareAdapter::new(config.dither))
        }
    }
}

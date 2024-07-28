use anyhow::Context;
use eink_home_display_rust::adapters::weather::open_weather::open_weather_weather_service::{OpenWeatherConfig, OpenWeatherWeatherServiceAdapter};
use eink_home_display_rust::application::Application;
use eink_home_display_rust::settings::{Settings, WeatherProvider};
use anyhow::Result;
use std::process;
use handlebars::Handlebars;
use tracing_subscriber::{EnvFilter, fmt};
use eink_home_display_rust::domain::models::GlanceData;
use eink_home_display_rust::domain::services::weather_service::WeatherService;
use eink_home_display_rust::domain::services::generate_screenshot::{capture_webpage, save_screenshot};
use eink_home_display_rust::domain::services::generate_1bit::{generate_1bit_image, save_1bit_array_as_png};
fn initialize_logging() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("debug"));

    fmt()
        .with_env_filter(env_filter)
        .init();
    // TODO: Figure out how to log the current log level as RUST_LOG should by able to override via try_from_default_env
    let current_level = tracing::level_filters::STATIC_MAX_LEVEL;
    tracing::info!("Logging initialized at {} level", current_level);
}
#[tokio::main]
async fn main() {
    initialize_logging();
    if let Err(e) = run().await {
        log::error!("Error: {}", e);
        for cause in e.chain().skip(1) {
            log::error!("Caused by: {}", cause);
        }
        process::exit(1);
    }
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



fn render_glance_data(glance_data: &GlanceData) -> Result<String> {
    let mut handlebars = Handlebars::new();
    handlebars.register_template_string("dashboard", include_str!("../templates/dashboard.html"))?;

    let rendered = handlebars.render("dashboard", glance_data)?;
    Ok(rendered)
}



async fn run() -> Result<()> {
    let settings = Settings::new().context("Failed to load settings")?;

    log::info!("Settings loaded successfully: {:?}", settings);
    let weather_service = setup_weather_service(&settings);
    let application = Application::new(weather_service);

    let result = application.run().await?;
    log::info!(
        "Yay!! your temperature is {}°C",
        result.weather_information().temperature()
    );
    let templated_html = render_glance_data(&result).context("Failed to render glance data")?;
    let screenshot_data = capture_webpage(templated_html).await?;
    save_screenshot(&screenshot_data, "./screenshot.png").context("Failed to save screenshot")?;
    let one_bit_data = generate_1bit_image(screenshot_data).context("Failed to generate 1-bit image")?;
    save_1bit_array_as_png(&one_bit_data, 800, 480, "./one_bit_screenshot.png")
        .context("Failed to save 1-bit screenshot")?;
    Ok(())
}

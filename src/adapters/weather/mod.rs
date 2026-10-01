pub mod no_op;
pub mod open_weather;

use crate::adapters::weather::no_op::no_op_weather_service::NoOpWeatherServiceAdapter;
use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use crate::config::weather::{WeatherConfig, WeatherProvider};
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

pub enum WeatherServiceImpl {
    OpenWeather(OpenWeatherWeatherServiceAdapter),
    NoOp(NoOpWeatherServiceAdapter),
}

impl WeatherService for WeatherServiceImpl {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<Option<WeatherInformation>> {
        match self {
            WeatherServiceImpl::OpenWeather(service) => {
                service.get_weather_for_location(location).await
            }
            WeatherServiceImpl::NoOp(service) => service.get_weather_for_location(location).await,
        }
    }
}

pub fn setup_weather_service(config: &WeatherConfig) -> WeatherServiceImpl {
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

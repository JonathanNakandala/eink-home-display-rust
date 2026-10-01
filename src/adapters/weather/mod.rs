pub mod no_op;
pub mod open_meteo;
pub mod open_weather;

use crate::adapters::weather::no_op::no_op_weather_service::NoOpWeatherServiceAdapter;
use crate::adapters::weather::open_meteo::open_meteo_weather_service::OpenMeteoWeatherServiceAdapter;
use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use anyhow::bail;

use crate::config::weather::{WeatherConfig, WeatherProvider, DEFAULT_OPEN_METEO_HOST_URL};
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

pub enum WeatherServiceImpl {
    OpenWeather(OpenWeatherWeatherServiceAdapter),
    OpenMeteo(OpenMeteoWeatherServiceAdapter),
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
            WeatherServiceImpl::OpenMeteo(service) => {
                service.get_weather_for_location(location).await
            }
            WeatherServiceImpl::NoOp(service) => service.get_weather_for_location(location).await,
        }
    }
}

pub fn setup_weather_service(config: &WeatherConfig) -> anyhow::Result<WeatherServiceImpl> {
    if !config.enabled {
        return Ok(WeatherServiceImpl::NoOp(NoOpWeatherServiceAdapter::new()));
    }
    match config.provider {
        WeatherProvider::OpenWeather => {
            let Some(open_weather) = &config.open_weather else {
                bail!("provider OpenWeather requires a [weather.open_weather] section");
            };
            Ok(WeatherServiceImpl::OpenWeather(
                OpenWeatherWeatherServiceAdapter::new(
                    open_weather.host_url.clone(),
                    open_weather.api_key.clone(),
                    reqwest::Client::new(),
                ),
            ))
        }
        WeatherProvider::OpenMeteo => {
            // The section is optional: no key is needed, so the default host works.
            let host_url = config
                .open_meteo
                .as_ref()
                .map_or_else(|| DEFAULT_OPEN_METEO_HOST_URL.to_owned(), |c| c.host_url.clone());
            Ok(WeatherServiceImpl::OpenMeteo(
                OpenMeteoWeatherServiceAdapter::new(host_url, reqwest::Client::new()),
            ))
        }
    }
}

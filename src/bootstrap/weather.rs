use anyhow::bail;

use crate::adapters::weather::no_op::no_op_weather_service::NoOpWeatherServiceAdapter;
use crate::adapters::weather::open_meteo::open_meteo_weather_service::OpenMeteoWeatherServiceAdapter;
use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use crate::adapters::weather::WeatherServiceImpl;
use crate::config::weather::{WeatherConfig, WeatherProvider, DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL, DEFAULT_OPEN_METEO_HOST_URL};

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
                    crate::adapters::http::client(),
                ),
            ))
        }
        WeatherProvider::OpenMeteo => {
            // The section is optional: no key is needed, so the default host works.
            let (host_url, air_quality_host_url, pollen) = match &config.open_meteo {
                Some(c) => (c.host_url.clone(), c.air_quality_host_url.clone(), c.pollen),
                None => (
                    DEFAULT_OPEN_METEO_HOST_URL.to_owned(),
                    DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL.to_owned(),
                    false,
                ),
            };
            Ok(WeatherServiceImpl::OpenMeteo(OpenMeteoWeatherServiceAdapter::new(
                host_url,
                air_quality_host_url,
                pollen,
                crate::adapters::http::client(),
            )))
        }
    }
}

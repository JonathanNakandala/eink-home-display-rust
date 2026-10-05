pub mod no_op;
pub mod open_meteo;
pub mod open_weather;

use crate::adapters::weather::no_op::no_op_weather_service::NoOpWeatherServiceAdapter;
use crate::adapters::weather::open_meteo::open_meteo_weather_service::OpenMeteoWeatherServiceAdapter;
use crate::adapters::weather::open_weather::open_weather_weather_service::OpenWeatherWeatherServiceAdapter;
use crate::domain::models::source_error::SourceError;

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
    ) -> Result<Option<WeatherInformation>, SourceError> {
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

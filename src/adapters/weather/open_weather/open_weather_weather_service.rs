use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

#[derive(derive_new::new)]
pub struct OpenWeatherWeatherServiceAdapter {}

impl WeatherService for OpenWeatherWeatherServiceAdapter {
    fn get_weather_for_location(&self, _location: Location) -> WeatherInformation {
        WeatherInformation::new(1)
    }
}
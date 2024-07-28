use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;

pub trait WeatherService {
    fn get_weather_for_location(&self, location: Location) -> WeatherInformation;
}
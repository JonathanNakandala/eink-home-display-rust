use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;

#[trait_variant::make(WeatherService : Send)]
pub trait LocalWeatherService {
    ///
    /// https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits.html
    async fn get_weather_for_location(&self, location: Location) -> WeatherInformation;
}

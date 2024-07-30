use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;

#[allow(async_fn_in_trait)]
pub trait LocalWeatherService {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<WeatherInformation>;
}

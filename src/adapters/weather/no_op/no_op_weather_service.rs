use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

#[derive(derive_new::new)]
pub struct NoOpWeatherServiceAdapter {}

impl WeatherService for NoOpWeatherServiceAdapter {
    async fn get_weather_for_location(
        &self,
        _location: Location,
    ) -> anyhow::Result<Option<WeatherInformation>> {
        log::debug!("Weather disabled via config, leaving it off the display");
        Ok(None)
    }
}

use crate::domain::models::location::Location;
use crate::domain::models::source_error::SourceError;
use crate::domain::models::weather::WeatherInformation;

#[allow(async_fn_in_trait)]
pub trait WeatherService {
    /// `None` when weather is switched off, so the display can leave it out.
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> Result<Option<WeatherInformation>, SourceError>;
}

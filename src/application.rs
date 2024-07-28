use crate::domain::models::location::Location;
use crate::domain::models::GlanceData;
use crate::domain::services::weather_service::WeatherService;

#[derive(derive_new::new)]
pub struct Application<WS: WeatherService> {
    weather_service: WS,
}

impl<WS: WeatherService> Application<WS> {
    pub fn run(&self) -> anyhow::Result<GlanceData> {
        let weather_information = self
            .weather_service
            .get_weather_for_location(Location::new());

        let glance_data = GlanceData::new(weather_information);

        Ok(glance_data)
    }
}

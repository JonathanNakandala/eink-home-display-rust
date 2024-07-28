use crate::domain::models::weather::WeatherInformation;
use serde::Serialize;
pub mod location;
pub mod weather;

#[derive(Serialize, derive_new::new)]
pub struct GlanceData {
    weather_information: WeatherInformation,
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}

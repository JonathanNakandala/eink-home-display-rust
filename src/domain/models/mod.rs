use crate::domain::models::weather::WeatherInformation;

pub mod weather;
pub mod location;

#[derive(derive_new::new)]
pub struct GlanceData {
    weather_information: WeatherInformation
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}
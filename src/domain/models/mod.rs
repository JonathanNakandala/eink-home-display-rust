use serde::Serialize;

use crate::domain::models::weather::WeatherInformation;

pub mod location;
pub mod weather;

pub mod image;

#[derive(Debug, derive_new::new, Serialize)]
pub struct GlanceData {
    weather_information: WeatherInformation,
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}

use crate::domain::models::weather::WeatherInformation;
use serde::Serialize;
pub mod location;
pub mod weather;

pub mod output;

pub use self::output::OutputResult;
#[derive(Debug, derive_new::new, Serialize)]
pub struct GlanceData {
    weather_information: WeatherInformation,
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}

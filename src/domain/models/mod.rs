use serde::Serialize;

use crate::domain::models::departures::DepartureService;
use crate::domain::models::weather::WeatherInformation;

pub mod arrival;
pub mod departures;
pub mod location;
pub mod stop_point;
pub mod weather;

pub mod image;
pub mod train;

#[derive(Debug, derive_new::new, Serialize)]
pub struct DepartureBoardData {
    name: String,
    services: Vec<DepartureService>,
}

#[derive(Debug, derive_new::new, Serialize)]
pub struct GlanceData {
    weather_information: WeatherInformation,
    departures: Vec<DepartureBoardData>,
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}

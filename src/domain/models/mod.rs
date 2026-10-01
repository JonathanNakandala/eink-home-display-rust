use serde::Serialize;

use crate::domain::models::departures::DepartureService;
use crate::domain::models::weather::WeatherInformation;

pub mod departures;
pub mod location;
pub mod weather;

pub mod image;
pub mod train;

#[derive(Debug, derive_new::new, Serialize)]
pub struct NationalRailInformation {
    #[serde(rename = "northboundTrains")]
    northbound_trains: Vec<DepartureService>,
    #[serde(rename = "southboundTrains")]
    southbound_trains: Vec<DepartureService>,
}

#[derive(Debug, derive_new::new, Serialize)]
pub struct GlanceData {
    weather_information: WeatherInformation,
    #[serde(rename = "nationalRail")]
    national_rail: NationalRailInformation,
}

impl GlanceData {
    pub fn weather_information(&self) -> &WeatherInformation {
        &self.weather_information
    }
}

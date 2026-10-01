use chrono::{DateTime, Local};
use serde::Serialize;

use crate::domain::models::departures::DepartureService;
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};

pub mod arrival;
pub mod departures;
pub mod display;
pub mod location;
pub mod stop_point;
pub mod weather;

pub mod image;
pub mod train;

#[derive(Debug, Clone, derive_new::new, Serialize)]
pub struct DepartureBoardData {
    name: String,
    services: Vec<DepartureService>,
}

/// The date and time as shown in the dashboard header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DateInfo {
    /// e.g. "14:05"
    current_time: String,
    /// e.g. "Wed"
    current_day: String,
    /// e.g. "1 Oct"
    current_date: String,
}

impl DateInfo {
    pub fn new(now: DateTime<Local>) -> Self {
        Self {
            current_time: now.format("%H:%M").to_string(),
            current_day: now.format("%a").to_string(),
            current_date: now.format("%-d %b").to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GlanceData {
    /// Left off the display when `None`.
    weather_information: Option<WeatherInformation>,
    departures: Vec<DepartureBoardData>,
    /// Rows the departures need in total, headings and "no services" lines included,
    /// so the template can size the text to fill the space.
    departure_lines: usize,
    date: DateInfo,
}

impl GlanceData {
    pub fn new(
        weather_information: Option<WeatherInformation>,
        departures: Vec<DepartureBoardData>,
        date: DateInfo,
    ) -> Self {
        let departure_lines = departures
            .iter()
            .map(|board| 1 + board.services.len().max(1))
            .sum();
        Self { weather_information, departures, departure_lines, date }
    }

    pub fn weather_information(&self) -> Option<&WeatherInformation> {
        self.weather_information.as_ref()
    }
}

impl GlanceData {
    /// Made-up but representative data (on time, delayed and cancelled
    /// services, plus a TfL-style countdown board) for previewing the layout offline.
    pub fn sample(now: DateTime<Local>) -> Self {
        let service = |time: &str, destination: &str, status: &str, delay: &str| {
            DepartureService::new(time.into(), destination.into(), status.into(), delay.into())
        };
        Self::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![
                DepartureBoardData::new(
                    "NORTHBOUND".into(),
                    vec![
                        service("14:12", "Welwyn Garden City", "On time", ""),
                        service("14:27", "Hertford North", "Delayed", "14:33"),
                        service("14:42", "Welwyn Garden City", "Cancelled", ""),
                        service("14:57", "Stevenage", "On time", ""),
                    ],
                ),
                DepartureBoardData::new(
                    "SOUTHBOUND".into(),
                    vec![
                        service("14:09", "Moorgate", "On time", ""),
                        service("14:24", "Kings Cross", "On time", ""),
                    ],
                ),
                // TfL boards only know the countdown, so status and delay are empty.
                DepartureBoardData::new(
                    "TURNPIKE LANE".into(),
                    vec![
                        service("due", "Piccadilly Cockfosters", "", ""),
                        service("3 min", "Piccadilly Heathrow Terminal 5", "", ""),
                        service("6 min", "Piccadilly Cockfosters", "", ""),
                        service("9 min", "Piccadilly Uxbridge", "", ""),
                    ],
                ),
            ],
            DateInfo::new(now),
        )
    }
}

use chrono::{DateTime, Duration, Local};
use serde::Serialize;

use crate::domain::models::departures::{countdown, DepartureService, DepartureStatus};
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};

pub mod arrival;
pub mod departures;
pub mod display;
pub mod location;
pub mod stop_point;
pub mod weather;

pub mod image;
pub mod train;

#[derive(Debug, Clone, Serialize)]
pub struct DepartureBoardData {
    name: String,
    /// The station's own name, shown beside the heading; empty when it adds nothing.
    station: String,
    services: Vec<DepartureService>,
}

impl DepartureBoardData {
    pub fn new(name: String, station: String, services: Vec<DepartureService>) -> Self {
        // A board already titled "Turnpike Lane" doesn't need the station repeated.
        let station = if station.eq_ignore_ascii_case(&name) { String::new() } else { station };
        Self { name, station, services }
    }
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
    /// Made-up but representative data (on time, delayed, cancelled and live
    /// services), timed relative to `now`, for previewing the layout offline.
    /// The first service on each board leaves after the journey to it: 5 minutes
    /// to Hornsey and 15 to Turnpike Lane.
    pub fn sample(now: DateTime<Local>) -> Self {
        let at = |minutes: i64| (now + Duration::minutes(minutes)).format("%H:%M").to_string();
        let timetabled = |minutes: i64, destination: &str, status, expected: Option<i64>| {
            let leaves_in = expected.unwrap_or(minutes);
            DepartureService::new(
                at(minutes),
                destination.into(),
                status,
                expected.map(at).unwrap_or_default(),
                countdown(leaves_in * 60),
            )
        };
        let live = |minutes: i64, destination: &str| {
            DepartureService::new(
                at(minutes),
                destination.into(),
                DepartureStatus::Live,
                String::new(),
                countdown(minutes * 60),
            )
        };
        Self::new(
            Some(WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)),
            vec![
                DepartureBoardData::new(
                    "NORTHBOUND".into(),
                    "Hornsey".into(),
                    vec![
                        timetabled(7, "Welwyn Garden City", DepartureStatus::OnTime, None),
                        timetabled(22, "Hertford North", DepartureStatus::Delayed, Some(28)),
                        // Cancelled and "late" trains have no countdown.
                        DepartureService::new(
                            at(37),
                            "Welwyn Garden City".into(),
                            DepartureStatus::Cancelled,
                            String::new(),
                            String::new(),
                        ),
                        DepartureService::new(
                            at(52),
                            "Stevenage".into(),
                            DepartureStatus::Delayed,
                            "late".into(),
                            String::new(),
                        ),
                    ],
                ),
                DepartureBoardData::new(
                    "SOUTHBOUND".into(),
                    "Hornsey".into(),
                    vec![
                        timetabled(6, "Moorgate", DepartureStatus::OnTime, None),
                        timetabled(21, "Kings Cross", DepartureStatus::OnTime, None),
                        timetabled(36, "Moorgate", DepartureStatus::OnTime, None),
                        timetabled(51, "Kings Cross", DepartureStatus::OnTime, None),
                    ],
                ),
                DepartureBoardData::new(
                    "TURNPIKE LANE".into(),
                    "Turnpike Lane".into(), // the same as the title, so it isn't repeated
                    vec![
                        live(16, "Cockfosters"),
                        live(16, "Oakwood"),
                        live(20, "Heathrow Terminal 5"),
                        live(24, "Cockfosters"),
                    ],
                ),
            ],
            DateInfo::new(now),
        )
    }
}

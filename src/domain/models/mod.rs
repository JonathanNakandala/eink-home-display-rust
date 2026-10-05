use chrono::{DateTime, Duration, Local, Timelike};
use serde::Serialize;

use crate::domain::models::air_quality::AirQuality;
use crate::domain::models::departures::{countdown, DepartureService, DepartureStatus};
use crate::domain::models::freshness::format_age;
use crate::domain::models::weather::{
    PrecipitationKind, PrecipitationOutlook, PrecipitationSlot, SunTimes, UvIndex,
    WeatherCondition, WeatherInformation,
};

pub mod air_quality;
pub mod arrival;
pub mod departures;
pub mod display;
pub mod freshness;
pub mod location;
pub mod pollen;
pub mod source_error;
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
    /// How old the services are when they are from an earlier fetch (e.g. "8 min"); empty when fresh.
    age: String,
    /// Nothing could be fetched and nothing recent enough is remembered.
    unavailable: bool,
    /// Why, in a few words (e.g. "timed out"), when unavailable.
    reason: String,
}

impl DepartureBoardData {
    pub fn new(name: String, station: String, services: Vec<DepartureService>) -> Self {
        // A board already titled "Turnpike Lane" doesn't need the station repeated.
        let station = if station.eq_ignore_ascii_case(&name) { String::new() } else { station };
        Self { name, station, services, age: String::new(), unavailable: false, reason: String::new() }
    }

    /// The board built from an earlier fetch, labelled with how old it is.
    pub fn from_earlier(name: String, station: String, services: Vec<DepartureService>, age: Duration) -> Self {
        Self { age: format_age(age), ..Self::new(name, station, services) }
    }

    pub fn unavailable(name: String, reason: &str) -> Self {
        Self { unavailable: true, reason: reason.to_owned(), ..Self::new(name, String::new(), Vec::new()) }
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
    /// How old the weather is when it is from an earlier fetch (e.g. "40 min"); empty when fresh.
    weather_age: String,
    /// The weather is switched on but couldn't be fetched, so say so instead of leaving a gap.
    weather_unavailable: bool,
    /// Why, in a few words (e.g. "key rejected"), when unavailable.
    weather_reason: String,
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
        Self {
            weather_information,
            departures,
            departure_lines,
            date,
            weather_age: String::new(),
            weather_unavailable: false,
            weather_reason: String::new(),
        }
    }

    /// The weather is from `age` ago.
    pub fn with_weather_age(mut self, age: Duration) -> Self {
        self.weather_age = format_age(age);
        self
    }

    pub fn with_weather_unavailable(mut self, reason: &str) -> Self {
        self.weather_unavailable = true;
        self.weather_reason = reason.to_owned();
        self
    }

    pub fn weather_information(&self) -> Option<&WeatherInformation> {
        self.weather_information.as_ref()
    }
}

impl GlanceData {
    /// The sample with some sources failed, to preview how that looks: the first board and the
    /// weather are from earlier fetches, and the last board is unavailable.
    pub fn sample_degraded(now: DateTime<Local>) -> Self {
        let mut data = Self::sample(now);
        if let Some(first) = data.departures.first_mut() {
            first.age = format_age(Duration::minutes(8));
        }
        if let Some(last) = data.departures.pop() {
            data.departures.push(DepartureBoardData::unavailable(last.name, "service error"));
        }
        data.departure_lines = data.departures.iter().map(|board| 1 + board.services.len().max(1)).sum();
        data.with_weather_age(Duration::minutes(40))
    }

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
        // Dry for the first three quarters of an hour, then a shower.
        let local = now.naive_local();
        let quarter = local - Duration::minutes(local.minute() as i64 % 15) - Duration::seconds(local.second() as i64);
        let showers = [0.0, 0.0, 0.0, 0.4, 1.2, 2.0, 1.0, 0.3]
            .iter()
            .enumerate()
            .map(|(i, rate)| {
                PrecipitationSlot::new(
                    quarter + Duration::minutes(15 * i as i64),
                    *rate,
                    0.0,
                    PrecipitationKind::Rain,
                )
            })
            .collect::<Vec<_>>();
        Self::new(
            Some(
                WeatherInformation::new(12, 8, 15, WeatherCondition::Clouds)
                    .with_precipitation(PrecipitationOutlook::from_slots(&showers, local))
                    .with_uv_index(Some(UvIndex::new(6.0)))
                    .with_feels_like(Some(9.0))
                    .with_sun(Some(SunTimes::new(
                        local.date().and_hms_opt(6, 51, 0).unwrap(),
                        local.date().and_hms_opt(18, 29, 0).unwrap(),
                    )))
                    .with_air_quality(AirQuality::new(
                        Some(62.0),
                        &[
                            ("PM2.5", Some(31.0)),
                            ("PM10", Some(62.0)),
                            ("NO₂", Some(27.0)),
                            ("Ozone", Some(18.0)),
                            ("SO₂", Some(4.0)),
                        ],
                    )),
            ),
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

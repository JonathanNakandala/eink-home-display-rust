use anyhow::Context;
use chrono::{Duration, NaiveDateTime, Utc};
use reqwest::Client;

use crate::adapters::weather::open_meteo::response::{OpenMeteoMinutelyResponse, OpenMeteoResponse};
use crate::domain::models::location::Location;
use crate::domain::models::weather::{
    PrecipitationKind, PrecipitationOutlook, PrecipitationSlot, WeatherCondition, WeatherInformation,
};
use crate::domain::services::weather_service::WeatherService;

/// Two hours of 15-minute slots.
const FORECAST_SLOTS: usize = 8;
const SLOTS_PER_HOUR: f64 = 4.0;

#[derive(derive_new::new)]
pub struct OpenMeteoWeatherServiceAdapter {
    host_url: String,
    client: Client,
}

impl WeatherService for OpenMeteoWeatherServiceAdapter {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<Option<WeatherInformation>> {
        let response = self
            .client
            .get(format!("{}/v1/forecast", self.host_url))
            .query(&[
                ("latitude", location.latitude.to_string()),
                ("longitude", location.longitude.to_string()),
                ("current", "temperature_2m,weather_code".to_owned()),
                ("daily", "temperature_2m_max,temperature_2m_min".to_owned()),
                ("minutely_15", "precipitation,snowfall,weather_code".to_owned()),
                ("forecast_minutely_15", FORECAST_SLOTS.to_string()),
                ("forecast_days", "1".to_owned()),
                ("timezone", "auto".to_owned()),
            ])
            .send()
            .await?
            .error_for_status()
            .context("Failed to fetch weather data")?;
        let body: OpenMeteoResponse = response
            .json()
            .await
            .context("Failed to parse weather data")?;
        log::debug!("Response body: {:#?}", &body);

        let max = body
            .daily
            .temperature_2m_max
            .first()
            .context("Weather response had no daily maximum")?;
        let min = body
            .daily
            .temperature_2m_min
            .first()
            .context("Weather response had no daily minimum")?;
        // Slot times are local to the location, so compare against its local time.
        let now = Utc::now().naive_utc() + Duration::seconds(body.utc_offset_seconds);
        let precipitation =
            PrecipitationOutlook::from_slots(&precipitation_slots(&body.minutely_15), now);
        Ok(Some(
            WeatherInformation::new(
                body.current.temperature_2m.round() as i8,
                min.round() as i8,
                max.round() as i8,
                condition_from_wmo_code(body.current.weather_code),
            )
            .with_precipitation(precipitation),
        ))
    }
}

/// Slots whose time can't be read are dropped; if that leaves gaps the chart is skipped.
fn precipitation_slots(minutely: &OpenMeteoMinutelyResponse) -> Vec<PrecipitationSlot> {
    let slots: Vec<_> = minutely
        .time
        .iter()
        .enumerate()
        .filter_map(|(i, time)| {
            let starts_at = NaiveDateTime::parse_from_str(time, "%Y-%m-%dT%H:%M").ok()?;
            // The API reports what fell in the slot; the chart shows a rate.
            let rate = |values: &[Option<f64>]| {
                values.get(i).copied().flatten().unwrap_or(0.0) * SLOTS_PER_HOUR
            };
            let precipitation = rate(&minutely.precipitation);
            let snowfall = rate(&minutely.snowfall);
            let code = minutely.weather_code.get(i).copied().flatten();
            let kind = code
                .and_then(precipitation_kind_from_wmo_code)
                // Wet but the code says cloud or fog: go by what is falling.
                .unwrap_or(if snowfall > 0.0 { PrecipitationKind::Snow } else { PrecipitationKind::Rain });
            Some(PrecipitationSlot::new(starts_at, precipitation, snowfall, kind))
        })
        .collect();
    if slots.len() == FORECAST_SLOTS { slots } else { Vec::new() }
}

fn precipitation_kind_from_wmo_code(code: u8) -> Option<PrecipitationKind> {
    match code {
        51 | 53 | 55 => Some(PrecipitationKind::Drizzle),
        56 | 57 => Some(PrecipitationKind::FreezingDrizzle),
        61 | 63 | 65 | 80..=82 => Some(PrecipitationKind::Rain),
        66 | 67 => Some(PrecipitationKind::FreezingRain),
        71..=77 | 85 | 86 => Some(PrecipitationKind::Snow),
        95..=99 => Some(PrecipitationKind::Thunderstorm),
        _ => None,
    }
}

/// See https://open-meteo.com/en/docs for the WMO code table.
fn condition_from_wmo_code(code: u8) -> WeatherCondition {
    match code {
        0 => WeatherCondition::Clear,
        51..=57 => WeatherCondition::Drizzle,
        61..=67 | 80..=82 => WeatherCondition::Rain,
        71..=77 | 85..=86 => WeatherCondition::Snow,
        95..=99 => WeatherCondition::Thunderstorm,
        // Cloud cover, and fog (45, 48), which has no icon of its own.
        _ => WeatherCondition::Clouds,
    }
}

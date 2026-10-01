use anyhow::Context;
use chrono::{Duration, NaiveDateTime, Utc};
use reqwest::Client;

use crate::adapters::weather::open_meteo::response::{
    OpenMeteoAirQualityResponse, OpenMeteoMinutelyResponse, OpenMeteoResponse,
};
use crate::domain::models::air_quality::AirQuality;
use crate::domain::models::location::Location;
use crate::domain::models::pollen::{Pollen, PollenType};
use crate::domain::models::weather::{
    PrecipitationKind, PrecipitationOutlook, PrecipitationSlot, UvIndex, WeatherCondition,
    WeatherInformation,
};
use crate::domain::services::weather_service::WeatherService;

/// Two hours of 15-minute slots.
const FORECAST_SLOTS: usize = 8;
const SLOTS_PER_HOUR: f64 = 4.0;

#[derive(derive_new::new)]
pub struct OpenMeteoWeatherServiceAdapter {
    host_url: String,
    air_quality_host_url: String,
    /// Also ask for pollen counts, which come from the air quality endpoint.
    pollen: bool,
    client: Client,
}

impl WeatherService for OpenMeteoWeatherServiceAdapter {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<Option<WeatherInformation>> {
        let (forecast, air_quality) = tokio::join!(
            self.get_forecast(&location),
            self.get_air_quality(&location)
        );
        // Air quality and pollen are a bonus: without them the rest of the weather still shows.
        let (air_quality, pollen) = air_quality.unwrap_or_else(|error| {
            log::warn!("Failed to get air quality: {error:#}");
            (None, None)
        });
        Ok(forecast?.map(|weather| weather.with_air_quality(air_quality).with_pollen(pollen)))
    }
}

impl OpenMeteoWeatherServiceAdapter {
    async fn get_air_quality(
        &self,
        location: &Location,
    ) -> anyhow::Result<(Option<AirQuality>, Option<Pollen>)> {
        let mut variables = "european_aqi,european_aqi_pm2_5,european_aqi_pm10,\
                             european_aqi_nitrogen_dioxide,european_aqi_ozone,european_aqi_sulphur_dioxide"
            .to_owned();
        if self.pollen {
            variables.push_str(
                ",alder_pollen,birch_pollen,grass_pollen,mugwort_pollen,olive_pollen,ragweed_pollen",
            );
        }
        let body: OpenMeteoAirQualityResponse = self
            .client
            .get(format!("{}/v1/air-quality", self.air_quality_host_url))
            .query(&[
                ("latitude", location.latitude.to_string()),
                ("longitude", location.longitude.to_string()),
                ("current", variables),
            ])
            .send()
            .await?
            .error_for_status()
            .context("Failed to fetch air quality data")?
            .json()
            .await
            .context("Failed to parse air quality data")?;
        log::debug!("Air quality response body: {:#?}", &body);

        let current = body.current;
        let pollen = self.pollen.then(|| {
            Pollen::new(&[
                (PollenType::Alder, current.alder_pollen),
                (PollenType::Birch, current.birch_pollen),
                (PollenType::Grass, current.grass_pollen),
                (PollenType::Mugwort, current.mugwort_pollen),
                (PollenType::Olive, current.olive_pollen),
                (PollenType::Ragweed, current.ragweed_pollen),
            ])
        });
        let air_quality = AirQuality::new(
            current.european_aqi,
            &[
                ("PM2.5", current.european_aqi_pm2_5),
                ("PM10", current.european_aqi_pm10),
                ("NO₂", current.european_aqi_nitrogen_dioxide),
                ("Ozone", current.european_aqi_ozone),
                ("SO₂", current.european_aqi_sulphur_dioxide),
            ],
        );
        Ok((air_quality, pollen.flatten()))
    }

    async fn get_forecast(&self, location: &Location) -> anyhow::Result<Option<WeatherInformation>> {
        let response = self
            .client
            .get(format!("{}/v1/forecast", self.host_url))
            .query(&[
                ("latitude", location.latitude.to_string()),
                ("longitude", location.longitude.to_string()),
                ("current", "temperature_2m,weather_code".to_owned()),
                ("daily", "temperature_2m_max,temperature_2m_min,uv_index_max".to_owned()),
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
            .with_precipitation(precipitation)
            .with_uv_index(body.daily.uv_index_max.first().copied().flatten().map(UvIndex::new)),
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

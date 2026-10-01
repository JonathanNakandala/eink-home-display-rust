use anyhow::Context;
use reqwest::Client;

use crate::adapters::weather::open_meteo::response::OpenMeteoResponse;
use crate::domain::models::location::Location;
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};
use crate::domain::services::weather_service::WeatherService;

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
        Ok(Some(WeatherInformation::new(
            body.current.temperature_2m.round() as i8,
            min.round() as i8,
            max.round() as i8,
            condition_from_wmo_code(body.current.weather_code),
        )))
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

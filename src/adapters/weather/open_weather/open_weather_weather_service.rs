use anyhow::Context;
use reqwest::Client;

use crate::adapters::weather::open_weather::response::OpenWeatherResponse;
use crate::domain::models::location::Location;
use crate::domain::models::weather::{WeatherCondition, WeatherInformation};
use crate::domain::services::weather_service::WeatherService;

#[derive(derive_new::new)]
pub struct OpenWeatherWeatherServiceAdapter {
    host_url: String,
    api_key: String,
    client: Client,
}

impl WeatherService for OpenWeatherWeatherServiceAdapter {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<Option<WeatherInformation>> {
        let url = format!(
            "{}/data/2.5/weather?lat={}&lon={}&appid={}&units=metric",
            self.host_url, location.latitude, location.longitude, self.api_key
        );

        let response = self.client.get(&url).send().await?;
        let response_body: OpenWeatherResponse = response
            .json()
            .await
            .context("Failed to fetch weather data")?;
        log::debug!("Response body: {:#?}", &response_body);

        let main = &response_body.main;
        // The current-conditions endpoint reports the spread across nearby stations,
        // not the day's forecast range.
        let condition = response_body
            .weather
            .first()
            .map_or(WeatherCondition::Clouds, |w| condition_from_id(w.id));
        Ok(Some(WeatherInformation::new(
            main.temp.round() as i8,
            main.temp_min.round() as i8,
            main.temp_max.round() as i8,
            condition,
        )))
    }
}

fn condition_from_id(id: u16) -> WeatherCondition {
    match id {
        200..=299 => WeatherCondition::Thunderstorm,
        300..=399 => WeatherCondition::Drizzle,
        500..=599 => WeatherCondition::Rain,
        600..=699 => WeatherCondition::Snow,
        800 => WeatherCondition::Clear,
        // Cloud cover, and the atmosphere group (mist, fog, haze...), which has no icon of its own.
        _ => WeatherCondition::Clouds,
    }
}

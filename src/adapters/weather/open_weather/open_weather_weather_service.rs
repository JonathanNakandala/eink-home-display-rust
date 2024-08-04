use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;

use crate::adapters::weather::open_weather::response::OpenWeatherResponse;
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::LocalWeatherService;

#[derive(Debug, Deserialize)]
pub struct OpenWeatherConfig {
    pub api_key: String,
}

pub struct OpenWeatherWeatherServiceAdapter {
    config: OpenWeatherConfig,
    client: Client,
}

impl OpenWeatherWeatherServiceAdapter {
    pub fn new(config: OpenWeatherConfig) -> Self {
        let client = Client::new();
        Self { config, client }
    }
}

impl LocalWeatherService for OpenWeatherWeatherServiceAdapter {
    async fn get_weather_for_location(
        &self,
        location: Location,
    ) -> anyhow::Result<WeatherInformation> {
        let url = format!(
            "https://api.openweathermap.org/data/2.5/weather?lat={}&lon={}&appid={}&units=metric",
            location.latitude, location.longitude, self.config.api_key
        );

        let response = self.client.get(&url).send().await?;
        let response_body: OpenWeatherResponse = response
            .json()
            .await
            .context("Failed to fetch weather data")?;
        log::debug!("Response body: {:#?}", &response_body);

        Ok(WeatherInformation::new(response_body.main.temp as i8))
    }
}

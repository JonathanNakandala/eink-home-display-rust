use anyhow::Context;
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::LocalWeatherService;
use serde::Deserialize;

use reqwest::Client;
#[derive(Debug, Deserialize)]
pub struct OpenWeatherConfig {
    pub api_key: String,
}
pub struct OpenWeatherWeatherServiceAdapter {
    config: OpenWeatherConfig,
    client: Client,
}
#[derive(Deserialize, Debug)]
#[allow(unused)]
struct OpenWeatherResponse {
    main: OpenWeatherMainResponse,
    name: String,
    visibility: i16,
    wind: OpenWeatherWindResponse,
}
#[derive(Deserialize, Debug)]
#[allow(unused)]
struct OpenWeatherWindResponse {
    speed: f32,
    deg: i16,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
struct OpenWeatherMainResponse {
    temp: f64,
    feels_like: f32,
    temp_min: f32,
    temp_max: f32,
    pressure: i32,
    humidity: i32,
    sea_level: i32,
    grnd_level: i32,
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
        let response_body: OpenWeatherResponse = response.json().await.context("Failed to fetch weather data")?;
        log::debug!("Response body: {:#?}", &response_body);

        Ok(WeatherInformation::new(response_body.main.temp as i8))
    }
}

use anyhow::Context;
use reqwest::Client;

use crate::adapters::weather::open_weather::response::OpenWeatherResponse;
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
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
    ) -> anyhow::Result<WeatherInformation> {
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

        Ok(WeatherInformation::new(response_body.main.temp as i8))
    }
}

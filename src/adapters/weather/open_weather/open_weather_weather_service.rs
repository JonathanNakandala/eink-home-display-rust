use serde::Deserialize;
use crate::domain::models::location::Location;
use crate::domain::models::weather::WeatherInformation;
use crate::domain::services::weather_service::WeatherService;

use reqwest::Client;
#[derive(Debug, Deserialize)]
pub struct OpenWeatherConfig {
    pub api_key: String,
    pub latitude: String,
    pub longitude: String
}
pub struct OpenWeatherWeatherServiceAdapter {
    config: OpenWeatherConfig,
    client: Client,
}
#[derive(Deserialize)]
#[allow(unused)]
struct OpenWeatherResponse {
    main: OpenWeatherMainResponse,
    name: String,
    visibility: i16,
    wind: OpenWeatherWindResponse
}
#[derive(Deserialize)]
#[allow(unused)]
struct OpenWeatherWindResponse {
    speed: f32,
    deg: i16
}

#[derive(Deserialize)]
#[allow(unused)]
struct OpenWeatherMainResponse {
    temp: f32,
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

    async fn fetch_weather_data(&self, _location: &Location) -> Result<String, reqwest::Error> {
        let url = format!(
            "https://api.openweathermap.org/data/2.5/weather?lat={}&lon={}&appid={}&units=metric",
            self.config.latitude, self.config.longitude, self.config.api_key
        );

        let response = self.client.get(&url).send().await?;
        let response_body = response.text().await?;
        log::debug!("Response body: {}", response_body);

        Ok(response_body)
    }


    fn parse_weather_data(&self, response_body: &str) -> Result<OpenWeatherResponse, serde_json::Error, > {
        let weather_data: OpenWeatherResponse = serde_json::from_str(response_body)?;
        Ok(weather_data)
    }
}
impl WeatherService for OpenWeatherWeatherServiceAdapter {
    async fn get_weather_for_location(&self, location: Location) -> WeatherInformation {
        match self.fetch_weather_data(&location).await {
            Ok(response_body) => {
                match self.parse_weather_data(&response_body) {
                    Ok(weather_data) => {
                        let temperature = weather_data.main.temp as i8;
                        log::info!("Temperature: {}°C", temperature);
                        WeatherInformation::new(temperature)
                    }
                    Err(error) => {
                        log::error!("Failed to parse weather data: {}", error);
                        WeatherInformation::new(0)
                    }
                }
            }
            Err(error) => {
                log::error!("Failed to fetch weather data: {}", error);
                WeatherInformation::new(0)
            }
        }
    }
}
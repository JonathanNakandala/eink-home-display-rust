use serde::Deserialize;
use serde_valid::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct OpenWeatherConfig {
    #[validate(max_length = 32)]
    #[validate(min_length = 32)]
    pub api_key: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct WeatherConfig {
    pub provider: WeatherProvider,
    #[validate]
    pub open_weather: OpenWeatherConfig,
}

#[derive(Debug, Deserialize)]
pub enum WeatherProvider {
    OpenWeather,
}

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_valid::Validate;

#[derive(Debug, Serialize, Deserialize, Validate, JsonSchema)]
pub struct WeatherConfig {
    /// When false no weather is fetched and the display shows none.
    pub enabled: bool,
    pub provider: WeatherProvider,
    #[validate]
    pub open_weather: OpenWeatherConfig,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub enum WeatherProvider {
    OpenWeather,
}

#[derive(Debug, Serialize, Deserialize, Validate, JsonSchema)]
pub struct OpenWeatherConfig {
    /// 32-character OpenWeather API key.
    #[validate(max_length = 32)]
    #[validate(min_length = 32)]
    #[schemars(length(min = 32, max = 32))]
    pub api_key: String,
    pub host_url: String
}

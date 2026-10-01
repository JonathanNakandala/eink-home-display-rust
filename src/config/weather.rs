use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_valid::Validate;

#[derive(Debug, Serialize, Deserialize, Validate, JsonSchema)]
pub struct WeatherConfig {
    /// When false no weather is fetched and the display shows none.
    pub enabled: bool,
    pub provider: WeatherProvider,
    /// Required when `provider = "OpenWeather"`.
    #[validate]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_weather: Option<OpenWeatherConfig>,
    /// Optional: Open-Meteo needs no key, so the default host is used when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_meteo: Option<OpenMeteoConfig>,
}

pub const DEFAULT_OPEN_METEO_HOST_URL: &str = "https://api.open-meteo.com";
pub const DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL: &str = "https://air-quality-api.open-meteo.com";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub enum WeatherProvider {
    /// Needs an API key and a One Call by Call subscription for the 4.0 API.
    OpenWeather,
    /// Free for non-commercial use, no key.
    OpenMeteo,
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

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenMeteoConfig {
    /// Open-Meteo API base URL, e.g. for a self-hosted instance or a paid plan.
    #[serde(default = "default_open_meteo_host_url")]
    pub host_url: String,
    /// Air quality is served from its own host.
    #[serde(default = "default_open_meteo_air_quality_host_url")]
    pub air_quality_host_url: String,
}

fn default_open_meteo_air_quality_host_url() -> String {
    DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL.to_owned()
}

fn default_open_meteo_host_url() -> String {
    DEFAULT_OPEN_METEO_HOST_URL.to_owned()
}

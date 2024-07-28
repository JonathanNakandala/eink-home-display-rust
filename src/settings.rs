use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Deserializer};
use thiserror::Error;
use log;
#[derive(Error, Debug)]
pub enum SettingValidationError {
    #[error("OpenWeather API Key must be 32 characters, got {0} characters")]
    OpenWeatherAPIKeyLength(usize)
}

pub fn validate_api_key<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    if s.len() != 32 {
        return Err(serde::de::Error::custom(SettingValidationError::OpenWeatherAPIKeyLength(s.len())));
    }
    Ok(s)
}

#[derive(Debug, Deserialize)]
pub struct OpenWeatherSettings {
    #[serde(deserialize_with = "validate_api_key")]
    pub api_key: String,
    pub latitude: String,
    pub longitude: String
}
#[derive(Debug, Deserialize)]
pub enum WeatherProvider {
    OpenWeather,
}
#[derive(Debug, Deserialize)]
pub struct WeatherSettings{
    pub provider: WeatherProvider,
    pub open_weather: OpenWeatherSettings
}
#[derive(Debug, Deserialize)]
pub struct Settings {
    pub weather: WeatherSettings
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
    log::debug!("Loading Settings");
    let s = Config::builder()
        // Default Config File
        .add_source(File::with_name("config/default"))
        .add_source(Environment::with_prefix("ink"))
        .build()?;
        s.try_deserialize()
    }
}
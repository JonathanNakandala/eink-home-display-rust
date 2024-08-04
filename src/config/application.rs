use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use serde_valid::Validate;

use crate::config::open_weather::WeatherConfig;

#[derive(Debug, Deserialize, Validate)]
pub struct ApplicationConfig {
    #[validate]
    pub weather: WeatherConfig,
    pub location: LocationConfig,
}

#[derive(Debug, Deserialize)]
pub struct LocationConfig {
    pub latitude: f64,
    pub longitude: f64,
}

impl ApplicationConfig {
    pub fn new() -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Default Config File
            .add_source(File::with_name("config/default"))
            .add_source(Environment::with_prefix("ink"))
            .build()?;
        s.try_deserialize()
    }
}

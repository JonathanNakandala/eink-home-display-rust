use std::path::{Path, PathBuf};

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use serde_valid::Validate;

use crate::config::open_weather::WeatherConfig;

#[derive(Debug, Deserialize, Validate)]
pub struct ApplicationConfig {
    #[validate]
    pub weather: WeatherConfig,
    pub location: LocationConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Deserialize)]
pub struct LocationConfig {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Deserialize)]
pub struct OutputConfig {
    pub save_directory: PathBuf,
}

impl ApplicationConfig {
    pub fn new(file_path: &Path) -> Result<Self, ConfigError> {
        let s = Config::builder()
            .add_source(File::from(file_path))
            .add_source(Environment::with_prefix("ink"))
            .build()?;
        s.try_deserialize()
    }
}

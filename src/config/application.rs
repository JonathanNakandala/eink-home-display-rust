use std::path::{Path, PathBuf};

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use serde_valid::Validate;

use crate::config::weather::WeatherConfig;

#[derive(Debug, Deserialize, Validate)]
pub struct ApplicationConfig {
    #[validate]
    pub weather: WeatherConfig,
    pub location: LocationConfig,
    pub file_store: FileStoreConfig,
    pub image: ImageConfig,
}

#[derive(Debug, Deserialize)]
pub struct LocationConfig {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Deserialize)]
pub struct FileStoreConfig {
    pub save_directory: PathBuf
}

#[derive(Debug, Deserialize)]
pub struct ImageConfig {
    pub height: u32,
    pub width: u32,
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

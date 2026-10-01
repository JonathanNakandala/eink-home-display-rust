use std::path::{Path, PathBuf};

use config::{Config, ConfigError, Environment, File};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_valid::Validate;

use crate::config::departures::{DepartureBoardConfig, ProvidersConfig};
use crate::config::weather::WeatherConfig;

#[derive(Debug, Serialize, Deserialize, Validate, JsonSchema)]
pub struct ApplicationConfig {
    #[validate]
    pub weather: WeatherConfig,
    /// Departure boards shown on the display, in order. Each picks its own provider.
    #[serde(default)]
    pub departures: Vec<DepartureBoardConfig>,
    /// Credentials and endpoints shared by all boards using a provider.
    #[serde(default)]
    pub providers: ProvidersConfig,
    pub location: LocationConfig,
    pub file_store: FileStoreConfig,
    pub image: ImageConfig,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LocationConfig {
    /// Decimal degrees, used for the weather lookup.
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileStoreConfig {
    /// Where rendered images are saved; empty for the working directory.
    pub save_directory: PathBuf
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ImageConfig {
    /// Display size in pixels.
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

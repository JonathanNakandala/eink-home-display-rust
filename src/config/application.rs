use std::path::{Path, PathBuf};

use config::{Config, ConfigError, Environment, File};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_valid::Validate;

use crate::config::cache::CacheConfig;
use crate::config::server::ServerConfig;
use crate::config::departures::{DepartureBoardConfig, ProvidersConfig};
use crate::config::weather::WeatherConfig;
use crate::domain::models::display::{Dither, ImageFormat};

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
    /// Re-creatable files kept between runs, such as a downloaded Chrome.
    #[serde(default)]
    pub cache: CacheConfig,
    pub display: DisplayConfig,
    /// How long an earlier result stands in for a source that has failed.
    #[serde(default)]
    pub stale_data: StaleDataConfig,
    /// How long a render, and each source in it, may take.
    #[serde(default)]
    pub limits: LimitsConfig,
    /// Serves the rendered image to displays that fetch it, like the reTerminal E1003.
    #[serde(default)]
    pub server: ServerConfig,
}

/// When a source fails, the dashboard shows what it said last time, labelled with its age, until it
/// is older than this; after that the part is shown as unavailable.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StaleDataConfig {
    /// Departures go out of date quickly, so this is short.
    #[serde(default = "default_departures_max_age")]
    pub departures_max_age_minutes: u32,
    #[serde(default = "default_weather_max_age")]
    pub weather_max_age_minutes: u32,
}

fn default_departures_max_age() -> u32 {
    15
}

fn default_weather_max_age() -> u32 {
    180
}

impl Default for StaleDataConfig {
    fn default() -> Self {
        Self {
            departures_max_age_minutes: default_departures_max_age(),
            weather_max_age_minutes: default_weather_max_age(),
        }
    }
}

impl From<&StaleDataConfig> for crate::application::MaxAge {
    fn from(config: &StaleDataConfig) -> Self {
        Self {
            departures: chrono::Duration::minutes(config.departures_max_age_minutes.into()),
            weather: chrono::Duration::minutes(config.weather_max_age_minutes.into()),
        }
    }
}

/// Time limits that keep one stuck part from stopping the dashboard updating.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LimitsConfig {
    /// A source (the weather, or one departures board) that hasn't answered by then counts as failed.
    #[serde(default = "default_source_timeout")]
    pub source_timeout_seconds: u32,
    /// A whole run, from fetching to the display, is abandoned after this long. Leave room for the
    /// sources' timeout plus Chrome (30 s, and a relaunch), and for a first-run Chrome download.
    #[serde(default = "default_render_deadline")]
    pub render_deadline_seconds: u32,
}

fn default_source_timeout() -> u32 {
    40
}

fn default_render_deadline() -> u32 {
    120
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            source_timeout_seconds: default_source_timeout(),
            render_deadline_seconds: default_render_deadline(),
        }
    }
}

impl From<&LimitsConfig> for crate::application::RenderLimits {
    fn from(config: &LimitsConfig) -> Self {
        Self {
            source_timeout: std::time::Duration::from_secs(config.source_timeout_seconds.into()),
            deadline: std::time::Duration::from_secs(config.render_deadline_seconds.into()),
        }
    }
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
pub struct DisplayConfig {
    /// Which display to render for and drive. Its size and colour depth are fixed by the hardware.
    pub kind: DisplayKind,
    /// How greys are reduced to the panel's levels.
    #[serde(default)]
    pub dither: Dither,
    /// The format the reTerminal E1003 is sent when it has no preference. Every format is published, and the
    /// display can ask for another with its `Accept` header. Ignored by displays driven directly.
    #[serde(default)]
    pub image_format: ImageFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub enum DisplayKind {
    /// Waveshare 7.5" V2, 800x480 black and white.
    WaveshareEpd7in5V2,
    /// Seeed reTerminal E1003, 10.3" 1872x1404 with 16 greys. Fetches its image from the `[server]`.
    ReTerminalE1003,
}

impl DisplayKind {
    pub const ALL: [DisplayKind; 2] = [DisplayKind::WaveshareEpd7in5V2, DisplayKind::ReTerminalE1003];

    /// Whether the display pulls its image over HTTP instead of being driven directly.
    pub fn fetches_image(self) -> bool {
        matches!(self, Self::ReTerminalE1003)
    }
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

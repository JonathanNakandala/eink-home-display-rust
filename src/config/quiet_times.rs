use std::path::Path;

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use serde_valid::Validate;

use crate::config::network_rail::NetworkRailConfig;

#[derive(Debug, Deserialize, Validate)]
pub struct QuietTimesConfig {
    #[validate]
    pub network_rail: NetworkRailConfig,
    pub rules: QuietTimesRulesConfig,
}

#[derive(Debug, Deserialize)]
pub struct QuietTimesRulesConfig {
    pub buffer_minutes: i64,
    /// Only used for display: how long a gap must be to be listed individually
    /// in the per-day breakdown. The single largest gap per day is always
    /// reported regardless of this threshold.
    pub min_gap_minutes: i64,
    /// Whether to restrict gap-finding to `weekday_window`/`weekend_window` at all;
    /// when false, the whole day (00:00-23:59:59) is considered.
    pub use_time_windows: bool,
    pub weekday_window: (String, String),
    pub weekend_window: (String, String),
}

impl QuietTimesConfig {
    pub fn new(file_path: &Path) -> Result<Self, ConfigError> {
        let s = Config::builder()
            .add_source(File::from(file_path))
            .add_source(Environment::with_prefix("ink_nr"))
            .build()?;
        s.try_deserialize()
    }
}

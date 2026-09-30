use std::path::PathBuf;

use serde::Deserialize;
use serde_valid::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct NetworkRailConfig {
    pub enabled: bool,
    pub feed_url: String,
    pub username: String,
    pub password: String,
    pub raw_file: PathBuf,
    pub cache_file: PathBuf,
    pub timing_points: Vec<String>,
}

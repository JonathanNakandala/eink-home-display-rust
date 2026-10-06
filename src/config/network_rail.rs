use std::path::PathBuf;

use secrecy::SecretString;
use serde::Deserialize;
use serde_valid::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct NetworkRailConfig {
    pub enabled: bool,
    pub feed_url: String,
    pub username: String,
    pub password: SecretString,
    pub raw_file: PathBuf,
    pub cache_file: PathBuf,
    pub timing_points: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_is_not_shown_when_the_config_is_printed() {
        let config: NetworkRailConfig = toml::from_str(
            r#"enabled = true
feed_url = "https://example.test/feed"
username = "me@example.test"
password = "hunter2-secret"
raw_file = "raw.gz"
cache_file = "cache.json"
timing_points = ["HRNSY"]"#,
        )
        .unwrap();
        let shown = format!("{config:?}");
        assert!(!shown.contains("hunter2-secret") && shown.contains("REDACTED"), "{shown}");
    }
}

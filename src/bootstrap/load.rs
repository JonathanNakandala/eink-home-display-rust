use std::path::Path;

use anyhow::Context;
use serde_valid::Validate;

use crate::config::application::ApplicationConfig;
use crate::config::quiet_times::QuietTimesConfig;

/// Reads the main configuration without checking its rules, for a program that uses only part of
/// it and checks that part itself (a disabled or unused section can hold placeholders).
pub fn load_application_config(path: &Path) -> anyhow::Result<ApplicationConfig> {
    ApplicationConfig::new(path).with_context(|| format!("Failed to load the configuration from {}", path.display()))
}

/// Reads the main configuration and checks the rules of everything in use. The error says which
/// file, and why.
pub fn load_valid_application_config(path: &Path) -> anyhow::Result<ApplicationConfig> {
    let config = load_application_config(path)?;
    config
        .validate()
        .with_context(|| format!("The configuration in {} is invalid", path.display()))?;
    Ok(config)
}

/// Reads and checks the quiet-times configuration.
pub fn load_quiet_times_config(path: &Path) -> anyhow::Result<QuietTimesConfig> {
    let config = QuietTimesConfig::new(path)
        .with_context(|| format!("Failed to load the configuration from {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("The configuration in {} is invalid", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes the example with its weather key replaced, and returns where.
    fn config_with_weather(name: &str, enabled: bool, api_key: &str) -> std::path::PathBuf {
        let mut config = ApplicationConfig::example();
        config.weather.enabled = enabled;
        config.weather.open_weather.as_mut().unwrap().api_key = api_key.to_owned();
        let path = std::env::temp_dir().join(format!("eink_load_test_{name}.toml"));
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_placeholder_key_is_fine_unless_weather_is_in_use() {
        let off = config_with_weather("off", false, "");
        assert!(load_valid_application_config(&off).is_ok());
        assert!(load_application_config(&off).is_ok());

        let on = config_with_weather("on", true, "");
        let error = load_valid_application_config(&on).unwrap_err();
        assert!(format!("{error:#}").contains("is invalid"), "{error:#}");
        // A program that doesn't use weather loads it all the same.
        assert!(load_application_config(&on).is_ok());
    }

    #[test]
    fn a_missing_file_says_which() {
        let error = load_application_config(Path::new("/nonexistent/eink.toml")).unwrap_err();
        assert!(format!("{error:#}").contains("/nonexistent/eink.toml"), "{error:#}");
    }
}

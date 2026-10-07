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
    config
        .location
        .zone()
        .with_context(|| format!("The [location] in {} is invalid", path.display()))?;
    if let Some(schedule) = &config.schedule {
        schedule
            .to_schedule()
            .with_context(|| format!("The [schedule] in {} is invalid", path.display()))?;
    }
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
        config.weather.open_weather.as_mut().unwrap().api_key = api_key.into();
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
    fn a_schedule_that_is_not_one_is_refused_and_a_good_one_is_kept() {
        use crate::config::schedule::ScheduleConfig;
        let write = |name: &str, cron: Vec<String>| {
            let mut config = ApplicationConfig::example();
            config.weather.enabled = false;
            config.schedule = Some(ScheduleConfig { cron });
            let path = std::env::temp_dir().join(format!("eink_load_test_{name}.toml"));
            std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
            path
        };
        let good = load_valid_application_config(&write("schedule_good", vec!["*/5 * * * *".into()])).unwrap();
        assert_eq!(good.schedule.unwrap().cron, ["*/5 * * * *"]);

        let error = load_valid_application_config(&write("schedule_bad", vec!["every day".into()])).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("[schedule]") && error.contains("every day"), "{error}");
    }

    #[test]
    fn a_timezone_is_checked_when_the_config_loads() {
        let write = |name: &str, timezone: Option<&str>| {
            let mut config = ApplicationConfig::example();
            config.weather.enabled = false;
            config.location.timezone = timezone.map(str::to_owned);
            let path = std::env::temp_dir().join(format!("eink_load_test_{name}.toml"));
            std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
            path
        };
        let good = load_valid_application_config(&write("zone_good", Some("America/New_York"))).unwrap();
        assert_eq!(good.location.timezone.as_deref(), Some("America/New_York"));
        // Optional: an older file without one still loads, and falls back to the host's at start-up.
        assert!(load_valid_application_config(&write("zone_none", None)).unwrap().location.timezone.is_none());

        let error = format!("{:#}", load_valid_application_config(&write("zone_bad", Some("Mars/Olympus"))).unwrap_err());
        assert!(error.contains("[location]") && error.contains("Mars/Olympus"), "{error}");
    }

    #[test]
    fn a_missing_file_says_which() {
        let error = load_application_config(Path::new("/nonexistent/eink.toml")).unwrap_err();
        assert!(format!("{error:#}").contains("/nonexistent/eink.toml"), "{error:#}");
    }
}

use std::path::Path;

use anyhow::Context;
use serde_valid::Validate;

use crate::config::application::ApplicationConfig;
use crate::config::quiet_times::QuietTimesConfig;

/// Reads and checks the main configuration. The error says which file, and why.
pub fn load_application_config(path: &Path) -> anyhow::Result<ApplicationConfig> {
    let config = ApplicationConfig::new(path)
        .with_context(|| format!("Failed to load the configuration from {}", path.display()))?;
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

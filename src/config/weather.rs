use schemars::JsonSchema;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_valid::Validate;

use crate::config::secret;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WeatherConfig {
    /// When false no weather is fetched and the display shows none.
    pub enabled: bool,
    pub provider: WeatherProvider,
    /// Required when `provider = "OpenWeather"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_weather: Option<OpenWeatherConfig>,
    /// Optional: Open-Meteo needs no key, so the default host is used when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_meteo: Option<OpenMeteoConfig>,
}

impl WeatherConfig {
    /// Checks the rules of the provider that is in use. Weather that is switched off, or the other
    /// provider's section, isn't checked: nothing reads its values, so a placeholder there is fine.
    pub fn validate_in_use(&self) -> Result<(), serde_valid::validation::Errors> {
        match (self.enabled, &self.provider, &self.open_weather) {
            (true, WeatherProvider::OpenWeather, Some(open_weather)) => open_weather.validate(),
            _ => Ok(()),
        }
    }
}

pub const DEFAULT_OPEN_METEO_HOST_URL: &str = "https://api.open-meteo.com";
pub const DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL: &str = "https://air-quality-api.open-meteo.com";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub enum WeatherProvider {
    /// Needs an API key and a One Call by Call subscription for the 4.0 API.
    OpenWeather,
    /// Free for non-commercial use, no key.
    OpenMeteo,
}

/// An OpenWeather key is exactly 32 characters; anything else is a placeholder or a typo.
fn api_key_length(key: &SecretString) -> Result<(), serde_valid::validation::Error> {
    // The message must not repeat the key, only how long it is.
    let length = key.expose_secret().chars().count();
    if length == 32 {
        Ok(())
    } else {
        Err(serde_valid::validation::Error::Custom(format!("the API key must be 32 characters, not {length}")))
    }
}

#[derive(Debug, Serialize, Deserialize, Validate, JsonSchema)]
pub struct OpenWeatherConfig {
    /// 32-character OpenWeather API key.
    #[serde(serialize_with = "secret::serialize")]
    #[validate(custom = api_key_length)]
    #[schemars(with = "String", length(min = 32, max = 32))]
    pub api_key: SecretString,
    pub host_url: String
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenMeteoConfig {
    /// Open-Meteo API base URL, e.g. for a self-hosted instance or a paid plan.
    #[serde(default = "default_open_meteo_host_url")]
    pub host_url: String,
    /// Air quality is served from its own host.
    #[serde(default = "default_open_meteo_air_quality_host_url")]
    pub air_quality_host_url: String,
    /// Also fetch pollen counts (Europe only). Off by default; they are not shown on the display yet.
    #[serde(default)]
    pub pollen: bool,
}

fn default_open_meteo_air_quality_host_url() -> String {
    DEFAULT_OPEN_METEO_AIR_QUALITY_HOST_URL.to_owned()
}

fn default_open_meteo_host_url() -> String {
    DEFAULT_OPEN_METEO_HOST_URL.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(enabled: bool, provider: WeatherProvider, api_key: &str) -> WeatherConfig {
        WeatherConfig {
            enabled,
            provider,
            open_weather: Some(OpenWeatherConfig {
                api_key: api_key.into(),
                host_url: "https://api.openweathermap.org".to_owned(),
            }),
            open_meteo: None,
        }
    }

    #[test]
    fn the_provider_in_use_is_checked() {
        assert!(config(true, WeatherProvider::OpenWeather, &"k".repeat(32)).validate_in_use().is_ok());
        assert!(config(true, WeatherProvider::OpenWeather, "").validate_in_use().is_err());
        assert!(config(true, WeatherProvider::OpenWeather, "too short").validate_in_use().is_err());
    }

    #[test]
    fn what_is_not_in_use_is_not_checked() {
        // Switched off, or the other provider is selected: a placeholder key is no problem.
        assert!(config(false, WeatherProvider::OpenWeather, "").validate_in_use().is_ok());
        assert!(config(true, WeatherProvider::OpenMeteo, "").validate_in_use().is_ok());
        let without_section = WeatherConfig { open_weather: None, ..config(true, WeatherProvider::OpenMeteo, "") };
        assert!(without_section.validate_in_use().is_ok());
    }
}

use serde::Serialize;

/// Serialised in lowercase to match the icon partial of the same name in the template.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WeatherCondition {
    Clear,
    Clouds,
    Drizzle,
    Rain,
    Thunderstorm,
    Snow,
}

#[derive(Debug, Serialize, derive_new::new, PartialEq)]
pub struct WeatherInformation {
    temperature: i8,
    min: i8,
    max: i8,
    condition: WeatherCondition,
}

impl WeatherInformation {
    pub fn temperature(&self) -> i8 {
        self.temperature
    }
}

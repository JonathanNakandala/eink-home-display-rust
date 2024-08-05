use serde::Serialize;

#[derive(Debug, Serialize, derive_new::new, PartialOrd, PartialEq)]
pub struct WeatherInformation {
    temperature: i8
}

impl WeatherInformation {
    pub fn temperature(&self) -> i8 {
        self.temperature
    }
}
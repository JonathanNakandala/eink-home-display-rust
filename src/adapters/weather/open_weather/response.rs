use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherResponse {
    pub main: OpenWeatherMainResponse,
    #[serde(default)]
    pub weather: Vec<OpenWeatherConditionResponse>,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct OpenWeatherMainResponse {
    pub temp: f64,
    pub temp_min: f64,
    pub temp_max: f64,
}

/// See https://openweathermap.org/weather-conditions for the id groups.
#[derive(Deserialize, Debug)]
pub struct OpenWeatherConditionResponse {
    pub id: u16,
}

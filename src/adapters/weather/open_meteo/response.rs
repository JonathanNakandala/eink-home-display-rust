use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct OpenMeteoResponse {
    pub current: OpenMeteoCurrentResponse,
    pub daily: OpenMeteoDailyResponse,
}

#[derive(Deserialize, Debug)]
pub struct OpenMeteoCurrentResponse {
    pub temperature_2m: f64,
    pub weather_code: u8,
}

/// One entry per requested forecast day; we ask for today only.
#[derive(Deserialize, Debug)]
pub struct OpenMeteoDailyResponse {
    pub temperature_2m_max: Vec<f64>,
    pub temperature_2m_min: Vec<f64>,
}

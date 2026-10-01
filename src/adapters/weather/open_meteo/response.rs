use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct OpenMeteoResponse {
    pub utc_offset_seconds: i64,
    pub current: OpenMeteoCurrentResponse,
    pub daily: OpenMeteoDailyResponse,
    /// Absent or partial data only costs the precipitation chart, not the weather.
    #[serde(default)]
    pub minutely_15: OpenMeteoMinutelyResponse,
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

/// Parallel arrays, one entry per 15-minute slot. Values are `null` where the model has none.
#[derive(Deserialize, Debug, Default)]
pub struct OpenMeteoMinutelyResponse {
    /// Local time, e.g. "2026-10-01T11:15".
    pub time: Vec<String>,
    /// mm in the 15 minutes.
    pub precipitation: Vec<Option<f64>>,
    /// cm in the 15 minutes.
    pub snowfall: Vec<Option<f64>>,
    pub weather_code: Vec<Option<u8>>,
}

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
    /// Only costs the UV line when absent or null.
    #[serde(default)]
    pub uv_index_max: Vec<Option<f64>>,
}

#[derive(Deserialize, Debug)]
pub struct OpenMeteoAirQualityResponse {
    pub current: OpenMeteoAirQualityCurrent,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct OpenMeteoAirQualityCurrent {
    pub european_aqi: Option<f64>,
    pub european_aqi_pm2_5: Option<f64>,
    pub european_aqi_pm10: Option<f64>,
    pub european_aqi_nitrogen_dioxide: Option<f64>,
    pub european_aqi_ozone: Option<f64>,
    pub european_aqi_sulphur_dioxide: Option<f64>,
    // Grains/m³; only present when asked for, and only in Europe.
    pub alder_pollen: Option<f64>,
    pub birch_pollen: Option<f64>,
    pub grass_pollen: Option<f64>,
    pub mugwort_pollen: Option<f64>,
    pub olive_pollen: Option<f64>,
    pub ragweed_pollen: Option<f64>,
}

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

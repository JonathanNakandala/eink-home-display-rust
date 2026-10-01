use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyResponse {
    #[serde(default)]
    pub stop_points: Vec<NearbyStop>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyStop {
    pub naptan_id: String,
    pub common_name: String,
    pub indicator: Option<String>,
    pub distance: Option<f64>,
    #[serde(default)]
    pub lines: Vec<Line>,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Deserialize)]
pub struct Line {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Deserialize)]
pub struct SearchMatch {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    pub line_name: String,
    #[serde(default)]
    pub destination_name: String,
    #[serde(default)]
    pub towards: String,
    #[serde(default)]
    pub platform_name: String,
    #[serde(default)]
    pub current_location: String,
    pub time_to_station: u32,
}

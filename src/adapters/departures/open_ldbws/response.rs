use serde::Deserialize;

/// Matches `StationBoardWithDetails` from the LDBWS REST API's swagger. Only
/// the fields the dashboard needs are modeled; the rest (calling points,
/// formation, etc.) are left to serde's default "ignore unknown fields".
#[derive(Debug, Deserialize, Default)]
pub struct StationBoard {
    #[serde(default, rename = "locationName")]
    pub location_name: Option<String>,
    #[serde(default, rename = "trainServices")]
    pub train_services: Option<Vec<Service>>,
}

#[derive(Debug, Deserialize)]
pub struct Service {
    /// Scheduled time of departure. Absent for an arrival-only entry (one
    /// with `sta`/`eta` instead), which isn't relevant to a departures board.
    pub std: Option<String>,
    pub etd: Option<String>,
    #[serde(default)]
    pub destination: Vec<ServiceLocation>,
}

#[derive(Debug, Deserialize)]
pub struct ServiceLocation {
    #[serde(rename = "locationName")]
    pub location_name: String,
}

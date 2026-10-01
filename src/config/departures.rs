use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_TFL_HOST_URL: &str = "https://api.tfl.gov.uk";

/// One `[[departures]]` entry: a titled board fed by a single provider.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DepartureBoardConfig {
    /// Heading shown above the board on the display.
    pub name: String,
    /// Set to false to skip this board without deleting it.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Maximum number of rows to show.
    #[serde(default = "default_rows")]
    pub rows: u8,
    /// Minutes it takes to get to the station or stop. Services leaving sooner than
    /// this are left off, since they can't be caught. National Rail boards support up to 119.
    #[serde(default)]
    pub travel_minutes: u16,
    #[serde(flatten)]
    pub source: DepartureSource,
}

/// Selected with `provider = "..."`; the remaining fields are provider-specific.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider")]
pub enum DepartureSource {
    /// National Rail live board between two CRS codes.
    OpenLdbws {
        /// CRS code of the station to show departures from, e.g. "HRN".
        from: String,
        /// CRS code the services must call at, e.g. "WIH".
        to: String,
    },
    /// TfL arrivals at a single stop (bus stop or station NaPTAN id).
    Tfl {
        /// NaPTAN id from `tfl_stops`, e.g. "940GZZLUTPN" (tube) or "490000173RC" (bus).
        stop_id: String,
    },
}

/// Credentials/endpoints shared by every board using that provider, under `[providers.*]`.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProvidersConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_ldbws: Option<OpenLdbwsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tfl: Option<TflConfig>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenLdbwsConfig {
    /// Consumer Key from your raildata.org.uk "Live Arrival and Departure Boards" subscription.
    pub api_key: String,
    /// Base path up to and including the operation name; the CRS code is appended as the final path segment.
    pub host_url: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TflConfig {
    /// Optional: anonymous requests work but are rate limited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_key: Option<String>,
    /// TfL Unified API base URL.
    #[serde(default = "default_tfl_host_url")]
    pub host_url: String,
}

fn default_enabled() -> bool {
    true
}

fn default_rows() -> u8 {
    4
}

fn default_tfl_host_url() -> String {
    DEFAULT_TFL_HOST_URL.to_owned()
}

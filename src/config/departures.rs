use serde::Deserialize;
use serde_valid::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct DeparturesConfig {
    pub enabled: bool,
    pub provider: DeparturesProvider,
    #[validate]
    pub open_ldbws: OpenLdbwsConfig,
    pub stations: StationsConfig,
}

#[derive(Debug, Deserialize)]
pub enum DeparturesProvider {
    OpenLdbws,
}

#[derive(Debug, Deserialize, Validate)]
pub struct OpenLdbwsConfig {
    pub api_key: String,
    pub host_url: String,
}

#[derive(Debug, Deserialize)]
pub struct StationsConfig {
    pub northbound_from: String,
    pub northbound_to: String,
    pub southbound_from: String,
    pub southbound_to: String,
}

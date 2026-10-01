pub mod no_op;
pub mod open_ldbws;

use no_op::no_op_departures_service::NoOpDeparturesServiceAdapter;
use open_ldbws::open_ldbws_departures_service::OpenLdbwsDeparturesServiceAdapter;

use crate::config::departures::{DeparturesConfig, DeparturesProvider};
use crate::domain::models::departures::DepartureService;
use crate::domain::services::departures_service::DeparturesService;

pub enum DeparturesServiceImpl {
    OpenLdbws(OpenLdbwsDeparturesServiceAdapter),
    NoOp(NoOpDeparturesServiceAdapter),
}

impl DeparturesService for DeparturesServiceImpl {
    async fn get_departures(
        &self,
        from: &str,
        to: &str,
        num_rows: u8,
    ) -> anyhow::Result<Vec<DepartureService>> {
        match self {
            DeparturesServiceImpl::OpenLdbws(service) => {
                service.get_departures(from, to, num_rows).await
            }
            DeparturesServiceImpl::NoOp(service) => {
                service.get_departures(from, to, num_rows).await
            }
        }
    }
}

pub fn setup_departures_service(config: &DeparturesConfig) -> DeparturesServiceImpl {
    if !config.enabled {
        return DeparturesServiceImpl::NoOp(NoOpDeparturesServiceAdapter::new());
    }
    match config.provider {
        DeparturesProvider::OpenLdbws => DeparturesServiceImpl::OpenLdbws(
            OpenLdbwsDeparturesServiceAdapter::new(
                config.open_ldbws.host_url.clone(),
                config.open_ldbws.api_key.clone(),
                reqwest::Client::new(),
            ),
        ),
    }
}

pub mod open_ldbws;
pub mod tfl;

use chrono::DateTime;
use chrono_tz::Tz;

pub use open_ldbws::open_ldbws_departures_service::OpenLdbwsDeparturesServiceAdapter;
pub use tfl::tfl_departures_service::TflDeparturesServiceAdapter;

use crate::domain::models::departures::Departures;
use crate::domain::models::source_error::SourceError;
use crate::domain::services::departures_service::DeparturesService;

pub enum DeparturesServiceImpl {
    OpenLdbws(OpenLdbwsDeparturesServiceAdapter),
    Tfl(TflDeparturesServiceAdapter),
}

impl DeparturesService for DeparturesServiceImpl {
    async fn get_departures(
        &self,
        num_rows: u8,
        now: DateTime<Tz>,
    ) -> Result<Departures, SourceError> {
        match self {
            DeparturesServiceImpl::OpenLdbws(service) => {
                service.get_departures(num_rows, now).await
            }
            DeparturesServiceImpl::Tfl(service) => service.get_departures(num_rows, now).await,
        }
    }
}

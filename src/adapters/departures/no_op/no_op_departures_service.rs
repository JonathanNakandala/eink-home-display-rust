use crate::domain::models::departures::DepartureService;
use crate::domain::services::departures_service::DeparturesService;

#[derive(derive_new::new)]
pub struct NoOpDeparturesServiceAdapter {}

impl DeparturesService for NoOpDeparturesServiceAdapter {
    async fn get_departures(
        &self,
        _from: &str,
        _to: &str,
        _num_rows: u8,
    ) -> anyhow::Result<Vec<DepartureService>> {
        log::debug!("Departures disabled via config, returning no scheduled trains");
        Ok(Vec::new())
    }
}

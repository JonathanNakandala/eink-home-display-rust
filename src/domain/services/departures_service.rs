use crate::domain::models::departures::DepartureService;

#[allow(async_fn_in_trait)]
pub trait DeparturesService {
    async fn get_departures(
        &self,
        from: &str,
        to: &str,
        num_rows: u8,
    ) -> anyhow::Result<Vec<DepartureService>>;
}

use crate::domain::models::departures::DepartureService;

/// A source of departures for one already-configured route or stop.
#[allow(async_fn_in_trait)]
pub trait DeparturesService {
    async fn get_departures(&self, num_rows: u8) -> anyhow::Result<Vec<DepartureService>>;
}

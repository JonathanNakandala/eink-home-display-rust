use crate::domain::models::arrival::Arrival;

#[allow(async_fn_in_trait)]
pub trait ArrivalsService {
    /// Predicted arrivals at a stop (bus stop or station), soonest first.
    async fn get_arrivals(&self, stop_id: &str) -> anyhow::Result<Vec<Arrival>>;
}

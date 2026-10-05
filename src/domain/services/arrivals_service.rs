use crate::domain::models::arrival::Arrival;
use crate::domain::models::source_error::SourceError;

#[allow(async_fn_in_trait)]
pub trait ArrivalsService {
    /// Predicted arrivals at a stop (bus stop or station), soonest first.
    async fn get_arrivals(&self, stop_id: &str) -> Result<Vec<Arrival>, SourceError>;
}

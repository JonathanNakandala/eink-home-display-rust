use crate::domain::models::location::Location;
use crate::domain::models::stop_point::{StopKind, StopPoint};

#[allow(async_fn_in_trait)]
pub trait StopPointService {
    /// Stops of `kind` within `radius_metres` of `location`, nearest first.
    async fn find_nearby(
        &self,
        location: Location,
        kind: StopKind,
        radius_metres: u32,
    ) -> anyhow::Result<Vec<StopPoint>>;

    /// Free-text search for stops of `kind` by name.
    async fn search_by_name(&self, query: &str, kind: StopKind) -> anyhow::Result<Vec<StopPoint>>;
}

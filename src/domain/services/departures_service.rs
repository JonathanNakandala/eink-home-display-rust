use chrono::DateTime;
use chrono_tz::Tz;

use crate::domain::models::departures::Departures;
use crate::domain::models::source_error::SourceError;

/// A source of departures for one already-configured route or stop.
#[allow(async_fn_in_trait)]
pub trait DeparturesService {
    /// `now` is passed in, rather than read here, so that times and countdowns
    /// agree with the rest of the display and can be tested.
    async fn get_departures(
        &self,
        num_rows: u8,
        now: DateTime<Tz>,
    ) -> Result<Departures, SourceError>;
}

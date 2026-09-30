use chrono::NaiveDate;

use crate::domain::models::train::TrainPassage;

#[allow(async_fn_in_trait)]
pub trait TrainScheduleService {
    async fn passages_on(&self, date: NaiveDate) -> anyhow::Result<Vec<TrainPassage>>;
}

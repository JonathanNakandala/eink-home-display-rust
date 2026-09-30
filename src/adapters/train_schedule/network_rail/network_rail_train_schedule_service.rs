use std::path::PathBuf;

use chrono::NaiveDate;

use crate::adapters::train_schedule::network_rail::schedule_cache;
use crate::domain::models::train::TrainPassage;
use crate::domain::services::train_schedule_service::TrainScheduleService;

#[derive(derive_new::new)]
pub struct NetworkRailTrainScheduleServiceAdapter {
    cache_file: PathBuf,
}

impl TrainScheduleService for NetworkRailTrainScheduleServiceAdapter {
    async fn passages_on(&self, date: NaiveDate) -> anyhow::Result<Vec<TrainPassage>> {
        let records = schedule_cache::load_cache(&self.cache_file)?;
        Ok(schedule_cache::passages_on_date(&records, date))
    }
}

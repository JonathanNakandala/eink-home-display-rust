use jiff::ToSpan;
use jiff::civil::Date;

use crate::domain::services::quiet_times_calculator::{QuietTimesCalculator, QuietTimesReport};
use crate::domain::services::train_schedule_service::TrainScheduleService;

#[derive(derive_new::new)]
pub struct QuietTimesApplication<TSS: TrainScheduleService> {
    train_schedule_service: TSS,
    calculator: QuietTimesCalculator,
}

impl<TSS: TrainScheduleService> QuietTimesApplication<TSS> {
    pub async fn run(&self, start: Date, days: u32) -> anyhow::Result<QuietTimesReport> {
        let mut daily_reports = Vec::new();
        for i in 0..days {
            let date = start.checked_add((i as i64).days())?;
            let today = self.train_schedule_service.passages_on(date).await?;
            let previous_day = self
                .train_schedule_service
                .passages_on(date.checked_sub(1.day())?)
                .await?;
            let combined: Vec<_> = today.into_iter().chain(previous_day).collect();
            daily_reports.push(self.calculator.report_for_day(date, &combined));
        }
        Ok(QuietTimesReport::new(daily_reports))
    }
}

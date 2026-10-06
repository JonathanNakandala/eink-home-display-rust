use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::models::schedule::Schedule;

/// When to refresh the dashboard. With this section the program keeps running and refreshes on the schedule;
/// without it (and without `--cron` or `--every`) it renders once and exits. `--cron` and `--every` on the
/// command line take the place of it, and `--once` ignores it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleConfig {
    /// Cron expressions in local time (minute, hour, day of month, month, day of week), e.g. `*/10 * * * *`.
    /// A refresh is due whenever any of them is, so each can cover its own hours and days. For example, every
    /// minute on weekday mornings, every 15 minutes the rest of the time on weekdays, and hourly at weekends:
    /// `["* 7-8 * * 1-5", "*/15 * * * 1-5", "0 8-21 * * 6,0"]`. The start-up log says what they mean in words
    /// and how many refreshes each day gets; each one is a wake of the display, which is what drains its battery.
    pub cron: Vec<String>,
}

impl ScheduleConfig {
    /// The schedule, or why an expression is not one.
    pub fn to_schedule(&self) -> anyhow::Result<Schedule> {
        Schedule::parse_crons(&self.cron)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expressions_make_one_schedule() {
        let config = ScheduleConfig { cron: vec!["*/10 * * * *".into(), "0 8 * * 6,0".into()] };
        assert_eq!(config.to_schedule().unwrap().describe().len(), 2);
    }

    #[test]
    fn a_bad_or_empty_list_is_an_error() {
        let error = ScheduleConfig { cron: vec!["every day".into()] }.to_schedule().unwrap_err().to_string();
        assert!(error.contains("every day"), "{error}");
        assert!(ScheduleConfig { cron: vec![] }.to_schedule().is_err());
    }
}

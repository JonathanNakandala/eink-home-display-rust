use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Local};
use croner::Cron;

/// When the periodic mode should run next.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Wall-clock aligned, in local time, e.g. `*/10 * * * *`.
    Cron(Cron),
    /// A fixed gap after each run starts.
    Every(Duration),
}

impl Schedule {
    pub fn parse_cron(expression: &str) -> anyhow::Result<Self> {
        let cron = expression
            .parse::<Cron>()
            .map_err(|e| anyhow!("Invalid cron expression {expression:?}: {e}"))?;
        Ok(Self::Cron(cron))
    }

    pub fn parse_every(period: &str) -> anyhow::Result<Self> {
        let period = humantime::parse_duration(period)
            .with_context(|| format!("Invalid interval {period:?}, expected e.g. 10m or 90s"))?;
        anyhow::ensure!(!period.is_zero(), "The interval must be longer than zero");
        Ok(Self::Every(period))
    }

    /// The first time strictly after `after` at which a run is due.
    pub fn next_after(&self, after: DateTime<Local>) -> anyhow::Result<DateTime<Local>> {
        match self {
            Self::Cron(cron) => cron
                .find_next_occurrence(&after, false)
                .map_err(|e| anyhow!("No next run time: {e}")),
            Self::Every(period) => Ok(after + *period),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    #[test]
    fn cron_aligns_to_the_clock() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        assert_eq!(schedule.next_after(at(8, 3, 20)).unwrap(), at(8, 10, 0));
        // Strictly after: a run starting on the slot doesn't repeat it.
        assert_eq!(schedule.next_after(at(8, 10, 0)).unwrap(), at(8, 20, 0));
    }

    #[test]
    fn every_adds_the_period() {
        let schedule = Schedule::parse_every("90s").unwrap();
        assert_eq!(schedule.next_after(at(8, 0, 0)).unwrap(), at(8, 1, 30));
    }

    #[test]
    fn rejects_bad_schedules() {
        assert!(Schedule::parse_cron("not a cron").is_err());
        assert!(Schedule::parse_every("soon").is_err());
        assert!(Schedule::parse_every("0s").is_err());
    }
}

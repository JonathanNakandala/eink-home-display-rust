use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;
use croner::Cron;

/// Counting a day's runs stops here, so a schedule with a tiny interval can't make it loop for ages.
const MAX_RUNS_COUNTED: usize = 100_000;

/// When the periodic mode should run next.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Wall-clock aligned, in local time, e.g. `*/10 * * * *`. With several, a run is due when any of them
    /// is, so each can cover its own hours and days (a busy morning, a quiet weekend).
    Cron(Vec<Cron>),
    /// A fixed gap after each run starts.
    Every(Duration),
}

impl Schedule {
    pub fn parse_cron(expression: &str) -> anyhow::Result<Self> {
        Self::parse_crons([expression])
    }

    /// One schedule from several cron expressions; a run is due whenever any of them is.
    pub fn parse_crons<S: AsRef<str>>(expressions: impl IntoIterator<Item = S>) -> anyhow::Result<Self> {
        let crons = expressions
            .into_iter()
            .map(|expression| {
                let expression = expression.as_ref();
                expression
                    .parse::<Cron>()
                    .map_err(|e| anyhow!("Invalid cron expression {expression:?}: {e}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        anyhow::ensure!(!crons.is_empty(), "A schedule needs at least one cron expression");
        Ok(Self::Cron(crons))
    }

    pub fn parse_every(period: &str) -> anyhow::Result<Self> {
        let period = humantime::parse_duration(period)
            .with_context(|| format!("Invalid interval {period:?}, expected e.g. 10m or 90s"))?;
        anyhow::ensure!(!period.is_zero(), "The interval must be longer than zero");
        Ok(Self::Every(period))
    }

    /// The first time strictly after `after` at which a run is due.
    pub fn next_after(&self, after: DateTime<Tz>) -> anyhow::Result<DateTime<Tz>> {
        match self {
            Self::Cron(crons) => {
                // One that has nothing left to give (a date that never comes) doesn't stop the others.
                let mut first_error = None;
                let mut earliest: Option<DateTime<Tz>> = None;
                for cron in crons {
                    match cron.find_next_occurrence(&after, false) {
                        Ok(next) => earliest = Some(earliest.map_or(next, |best| best.min(next))),
                        Err(e) => {
                            first_error.get_or_insert(e);
                        }
                    }
                }
                earliest.ok_or_else(|| match first_error {
                    Some(e) => anyhow!("No next run time: {e}"),
                    None => anyhow!("No next run time"),
                })
            }
            Self::Every(period) => chrono::Duration::from_std(*period)
                .ok()
                .and_then(|period| after.checked_add_signed(period))
                .ok_or_else(|| anyhow!("The interval {period:?} is too long to schedule")),
        }
    }

    /// The schedule in words, one line per cron expression, for a person to check it says what they meant.
    pub fn describe(&self) -> Vec<String> {
        match self {
            Self::Cron(crons) => crons.iter().map(|cron| format!("{}  {}", cron.as_str(), cron.describe())).collect(),
            Self::Every(period) => vec![format!("Every {}", humantime::format_duration(*period))],
        }
    }

    /// How many runs fall on `day`, a calendar day in `zone`. Stops counting at a large number, so a tiny interval
    /// gives that number, not the true one.
    pub fn runs_on(&self, day: NaiveDate, zone: Tz) -> anyhow::Result<usize> {
        let start = start_of_day(day, zone).with_context(|| format!("Can't tell when {day} starts"))?;
        let end = day
            .succ_opt()
            .and_then(|next| start_of_day(next, zone))
            .with_context(|| format!("Can't tell when {day} ends"))?;
        let mut count = 0;
        // Strictly after, so a run at the very start of the day is counted.
        let mut at = start - chrono::Duration::seconds(1);
        while count < MAX_RUNS_COUNTED {
            at = self.next_after(at)?;
            if at >= end {
                break;
            }
            count += 1;
        }
        Ok(count)
    }

    /// The next `count` runs after `after`.
    pub fn upcoming(&self, after: DateTime<Tz>, count: usize) -> anyhow::Result<Vec<DateTime<Tz>>> {
        let mut at = after;
        (0..count)
            .map(|_| {
                at = self.next_after(at)?;
                Ok(at)
            })
            .collect()
    }
}

/// Midnight at the start of `day`; in a zone where the clocks change at midnight, the first moment that exists.
fn start_of_day(day: NaiveDate, zone: Tz) -> Option<DateTime<Tz>> {
    let midnight = day.and_hms_opt(0, 0, 0)?;
    zone.from_local_datetime(&midnight)
        .earliest()
        .or_else(|| zone.from_local_datetime(&(midnight + chrono::Duration::hours(1))).earliest())
}

#[cfg(test)]
mod tests {
    use chrono_tz::Europe::London;
    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
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
    fn an_interval_too_long_to_add_is_an_error_not_a_panic() {
        let schedule = Schedule::parse_every("999999999y").unwrap();
        assert!(schedule.next_after(at(8, 0, 0)).is_err());
        assert!(Schedule::Every(Duration::MAX).next_after(at(8, 0, 0)).is_err());
    }

    #[test]
    fn rejects_bad_schedules() {
        assert!(Schedule::parse_cron("not a cron").is_err());
        assert!(Schedule::parse_every("soon").is_err());
        assert!(Schedule::parse_every("0s").is_err());
    }

    /// A weekday with a busy morning and lunchtime, a quiet day, and an hourly weekend with no nights.
    const WEEK: [&str; 4] = [
        "* 7-8,11-12 * * 1-5",
        "*/5 9-10,13-21 * * 1-5",
        "*/15 22-23,0-6 * * 1-5",
        "0 8-21 * * 6,0",
    ];

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, d).unwrap()
    }

    #[test]
    fn several_expressions_run_whichever_is_next() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        // Monday 2026-06-15.
        assert_eq!(schedule.next_after(at(6, 59, 0)).unwrap(), at(7, 0, 0));
        assert_eq!(schedule.next_after(at(7, 0, 0)).unwrap(), at(7, 1, 0), "every minute in the morning");
        assert_eq!(schedule.next_after(at(8, 59, 30)).unwrap(), at(9, 0, 0));
        assert_eq!(schedule.next_after(at(9, 0, 0)).unwrap(), at(9, 5, 0), "every five minutes after it");
        assert_eq!(schedule.next_after(at(21, 57, 0)).unwrap(), at(22, 0, 0));
        assert_eq!(schedule.next_after(at(22, 0, 0)).unwrap(), at(22, 15, 0), "every fifteen at night");
    }

    #[test]
    fn the_week_has_the_runs_it_was_written_for() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        // 4 h a minute, 11 h every five minutes, 9 h every fifteen.
        assert_eq!(schedule.runs_on(day(15), London).unwrap(), 240 + 132 + 36);
        assert_eq!(schedule.runs_on(day(19), London).unwrap(), 240 + 132 + 36, "Friday");
        // Hourly from 08:00 to 21:00, and nothing in the small hours or the evening.
        assert_eq!(schedule.runs_on(day(20), London).unwrap(), 14, "Saturday");
        assert_eq!(schedule.runs_on(day(21), London).unwrap(), 14, "Sunday");
    }

    #[test]
    fn runs_are_counted_for_the_other_kinds_too() {
        assert_eq!(Schedule::parse_cron("*/10 * * * *").unwrap().runs_on(day(15), London).unwrap(), 144);
        assert_eq!(Schedule::parse_every("1h").unwrap().runs_on(day(15), London).unwrap(), 24);
        // A tiny interval is counted up to a limit, not for ever.
        assert_eq!(Schedule::parse_every("1ms").unwrap().runs_on(day(15), London).unwrap(), MAX_RUNS_COUNTED);
    }

    #[test]
    fn the_next_runs_are_listed_in_order() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        let runs = schedule.upcoming(at(8, 58, 0), 4).unwrap();
        assert_eq!(runs, vec![at(8, 59, 0), at(9, 0, 0), at(9, 5, 0), at(9, 10, 0)]);
    }

    #[test]
    fn a_schedule_is_described_in_words_one_line_each() {
        let lines = Schedule::parse_crons(["*/10 * * * *", "0 8-21 * * 6,0"]).unwrap().describe();
        assert_eq!(
            lines,
            [
                "*/10 * * * *  At every 10 minutes.",
                "0 8-21 * * 6,0  At minute 0, of hour 8-21, on Sunday and Saturday.",
            ]
        );
        assert_eq!(Schedule::parse_every("90s").unwrap().describe(), ["Every 1m 30s"]);
    }

    #[test]
    fn a_list_needs_every_expression_to_be_valid_and_at_least_one() {
        let error = Schedule::parse_crons(["*/10 * * * *", "nonsense"]).unwrap_err().to_string();
        assert!(error.contains("nonsense"), "{error}");
        assert!(Schedule::parse_crons(Vec::<String>::new()).is_err());
    }

    #[test]
    fn an_expression_with_nothing_left_does_not_stop_the_others() {
        // 31 February never comes; the other still does.
        let schedule = Schedule::parse_crons(["0 0 31 2 *", "*/10 * * * *"]).unwrap();
        assert_eq!(schedule.next_after(at(8, 3, 0)).unwrap(), at(8, 10, 0));
        assert!(Schedule::parse_crons(["0 0 31 2 *"]).unwrap().next_after(at(8, 3, 0)).is_err());
    }
}

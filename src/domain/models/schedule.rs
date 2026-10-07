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

    // ---- Clock changes, in several zones ----

    use chrono::{Offset, Utc};
    use chrono_tz::{Asia, Australia, Europe, America, Pacific};

    fn offset_seconds(zone: &Tz, at: DateTime<Utc>) -> i32 {
        at.with_timezone(zone).offset().fix().local_minus_utc()
    }

    /// The start of the minute at or after `at`: a cron expression has only minutes.
    fn ceil_to_minute(at: DateTime<Utc>) -> DateTime<Utc> {
        let seconds = at.timestamp() + i64::from(at.timestamp_subsec_nanos() > 0);
        DateTime::from_timestamp((seconds + 59).div_euclid(60) * 60, 0).unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// The instants in 2026 at which `zone`'s offset changes, to the minute, and by how many seconds.
    fn changes_in_2026(zone: Tz) -> Vec<(DateTime<Utc>, i32)> {
        let mut found = Vec::new();
        let mut t = utc(2026, 1, 1, 0, 0);
        while t < utc(2027, 1, 1, 0, 0) {
            let next = t + chrono::Duration::hours(1);
            let (a, b) = (offset_seconds(&zone, t), offset_seconds(&zone, next));
            if a != b {
                let mut minute = t;
                while offset_seconds(&zone, minute) == a {
                    minute += chrono::Duration::minutes(1);
                }
                found.push((minute, b - a));
            }
            t = next;
        }
        found
    }

    /// What a run means: every minute, in UTC, whose local time matches. Slow and plainly right.
    fn reference(cron: &Cron, zone: Tz, after: DateTime<Utc>, until: DateTime<Utc>) -> Vec<DateTime<Utc>> {
        let mut minute = ceil_to_minute(after);
        if minute <= after {
            minute += chrono::Duration::minutes(1);
        }
        let mut found = Vec::new();
        while minute <= until {
            if cron.is_time_matching(&minute.with_timezone(&zone)).unwrap() {
                found.push(minute);
            }
            minute += chrono::Duration::minutes(1);
        }
        found
    }

    const ZONES: [Tz; 8] = [
        Europe::London,
        America::New_York,
        Australia::Sydney,
        Australia::Lord_Howe,
        America::Santiago,
        Pacific::Chatham,
        Asia::Colombo,
        Asia::Kolkata,
    ];

    #[test]
    fn a_run_every_so_often_keeps_its_rhythm_through_every_clock_change_in_every_zone() {
        let mut checked_changes = 0;
        for zone in ZONES {
            for expression in ["*/10 * * * *", "*/15 * * * *", "0 * * * *", "*/20 0-6 * * *", "7 * * * *"] {
                let cron: Cron = expression.parse().unwrap();
                let schedule = Schedule::parse_cron(expression).unwrap();
                for (change, _) in changes_in_2026(zone) {
                    checked_changes += 1;
                    let from = change - chrono::Duration::hours(30);
                    let until = change + chrono::Duration::hours(30);
                    let expected = reference(&cron, zone, from, until);

                    let mut got = Vec::new();
                    let mut at = from.with_timezone(&zone);
                    loop {
                        let next = schedule.next_after(at).unwrap();
                        assert!(next > at, "{zone} {expression}: {next} is not after {at}");
                        if next.with_timezone(&Utc) > until {
                            break;
                        }
                        got.push(next.with_timezone(&Utc));
                        at = next;
                    }
                    assert_eq!(got, expected, "{zone} {expression} around {change}");
                }
            }
        }
        // The zones above have both kinds of change, so this did look at clocks going back as well as forward.
        assert_eq!(checked_changes, 5 * 12, "six zones with two changes each, for five expressions");
    }

    #[test]
    fn from_any_moment_the_next_run_is_the_first_match_after_it() {
        // Starting between slots, on them, and a second either side, around the UK's autumn change.
        let zone = Europe::London;
        let cron: Cron = "*/10 * * * *".parse().unwrap();
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let change = utc(2026, 10, 25, 1, 0);
        for offset_seconds in (-4000..4000).step_by(37) {
            let start = change + chrono::Duration::seconds(offset_seconds);
            let expected = reference(&cron, zone, start, start + chrono::Duration::hours(2))[0];
            let got = schedule.next_after(start.with_timezone(&zone)).unwrap().with_timezone(&Utc);
            assert_eq!(got, expected, "from {start}");
        }
    }

    #[test]
    fn the_uk_autumn_change_has_no_gap() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = Europe::London;
        // 00:50 BST on 25 October 2026; the clocks go back at 02:00 BST to 01:00 GMT.
        let mut at = zone.with_ymd_and_hms(2026, 10, 25, 0, 50, 0).unwrap();
        let mut previous = at;
        for _ in 0..16 {
            at = schedule.next_after(at).unwrap();
            assert_eq!((at - previous).num_minutes(), 10, "{previous} to {at}");
            previous = at;
        }
        // It went through 01:50 BST and on into 01:00 GMT, an hour of real time later.
        assert_eq!(at.format("%H:%M %Z").to_string(), "02:30 GMT");
    }

    #[test]
    fn a_day_with_a_clock_change_has_the_runs_its_length_allows() {
        let runs = |expression: &str, zone: Tz, y, m, d| {
            Schedule::parse_cron(expression).unwrap().runs_on(NaiveDate::from_ymd_opt(y, m, d).unwrap(), zone).unwrap()
        };
        // 25 hours in the UK on 25 October, 23 on 29 March.
        assert_eq!(runs("*/10 * * * *", Europe::London, 2026, 10, 25), 150);
        assert_eq!(runs("*/10 * * * *", Europe::London, 2026, 3, 29), 138);
        assert_eq!(runs("0 * * * *", Europe::London, 2026, 10, 25), 25);
        assert_eq!(runs("0 * * * *", Europe::London, 2026, 3, 29), 23);
        assert_eq!(runs("*/10 * * * *", Europe::London, 2026, 10, 24), 144);
        // The southern hemisphere changes the other way round: Sydney goes back on 5 April, forward on 4 October.
        assert_eq!(runs("*/10 * * * *", Australia::Sydney, 2026, 4, 5), 150);
        assert_eq!(runs("*/10 * * * *", Australia::Sydney, 2026, 10, 4), 138);
        // Lord Howe changes by half an hour.
        assert_eq!(runs("*/10 * * * *", Australia::Lord_Howe, 2026, 4, 5), 147);
        assert_eq!(runs("*/10 * * * *", Australia::Lord_Howe, 2026, 10, 4), 141);
        // Colombo and Kolkata have no change, and an offset of a half hour: every day is the same.
        for zone in [Asia::Colombo, Asia::Kolkata] {
            for (m, d) in [(3, 29), (4, 5), (10, 4), (10, 25)] {
                assert_eq!(runs("*/10 * * * *", zone, 2026, m, d), 144, "{zone} {m}-{d}");
            }
        }
    }

    /// The convention of Vixie cron, and what `croner` does: a job at a fixed time runs once even when that time is
    /// on the clock twice, while a job that is "every so often" keeps its rhythm through the repeated hour (the
    /// brute-force test above). A fixed time that doesn't exist, in the hour that is skipped, runs at the change.
    #[test]
    fn a_fixed_time_runs_once_in_a_repeated_hour_and_one_that_was_skipped_runs_at_the_change() {
        let zone = Europe::London;
        let daily = Schedule::parse_cron("30 1 * * *").unwrap();
        // Autumn: 01:30 is on the clock twice, and the daily run happens once, the first time.
        let first = daily.next_after(zone.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap()).unwrap();
        assert_eq!(first.format("%d %H:%M %Z").to_string(), "25 01:30 BST");
        assert_eq!(daily.next_after(first).unwrap().format("%d %H:%M %Z").to_string(), "26 01:30 GMT");
        assert_eq!(
            Schedule::parse_cron("30 1 * * *").unwrap().runs_on(NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), zone).unwrap(),
            1
        );
        // Spring: 01:30 doesn't exist, and the run happens when the clocks change.
        let spring = daily.next_after(zone.with_ymd_and_hms(2026, 3, 29, 0, 0, 0).unwrap()).unwrap();
        assert_eq!(spring.with_timezone(&Utc), utc(2026, 3, 29, 1, 0));
        // And the day after is the normal one.
        assert_eq!(daily.next_after(spring).unwrap().format("%d %H:%M").to_string(), "30 01:30");
    }

    #[test]
    fn a_result_is_never_the_moment_it_was_asked_from() {
        // Where the clocks go forward, the answer used to name the instant it started from.
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = Europe::London;
        let mut at = utc(2026, 3, 29, 0, 30).with_timezone(&zone);
        for _ in 0..12 {
            let next = schedule.next_after(at).unwrap();
            assert!(next > at, "{at} then {next}");
            at = next;
        }
    }

    #[test]
    fn a_span_with_no_change_in_it_costs_nothing_extra_and_is_unchanged() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = Asia::Colombo;
        let at = zone.with_ymd_and_hms(2026, 6, 15, 8, 3, 20).unwrap();
        assert_eq!(schedule.next_after(at).unwrap(), zone.with_ymd_and_hms(2026, 6, 15, 8, 10, 0).unwrap());
    }

    #[test]
    fn several_expressions_take_the_earliest_across_a_repeated_hour() {
        let schedule = Schedule::parse_crons(["0 1 * * *", "*/30 * * * *"]).unwrap();
        let zone = Europe::London;
        let mut at = zone.with_ymd_and_hms(2026, 10, 25, 0, 40, 0).unwrap();
        let mut gaps = Vec::new();
        for _ in 0..5 {
            let next = schedule.next_after(at).unwrap();
            gaps.push((next - at).num_minutes());
            at = next;
        }
        // 01:00 BST, 01:30 BST, 01:00 GMT, 01:30 GMT, 02:00 GMT.
        assert_eq!(gaps, [20, 30, 30, 30, 30]);
    }
}

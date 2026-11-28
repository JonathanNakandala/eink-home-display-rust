use std::time::Duration;

use anyhow::{Context, anyhow};
use croner::Cron;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, ZonedDateTime};

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
    pub fn parse_crons<S: AsRef<str>>(
        expressions: impl IntoIterator<Item = S>,
    ) -> anyhow::Result<Self> {
        let crons = expressions
            .into_iter()
            .map(|expression| {
                let expression = expression.as_ref();
                expression
                    .parse::<Cron>()
                    .map_err(|e| anyhow!("Invalid cron expression {expression:?}: {e}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        anyhow::ensure!(
            !crons.is_empty(),
            "A schedule needs at least one cron expression"
        );
        Ok(Self::Cron(crons))
    }

    pub fn parse_every(period: &str) -> anyhow::Result<Self> {
        let period = parse_interval(period)
            .with_context(|| format!("Invalid interval {period:?}, expected e.g. 10m or 90s"))?;
        anyhow::ensure!(!period.is_zero(), "The interval must be longer than zero");
        Ok(Self::Every(period))
    }

    /// The first time strictly after `after` at which a run is due.
    pub fn next_after(&self, after: ZonedDateTime) -> anyhow::Result<ZonedDateTime> {
        match self {
            Self::Cron(crons) => {
                // One that has nothing left to give (a date that never comes) doesn't stop the others.
                let mut first_error = None;
                let mut earliest: Option<ZonedDateTime> = None;
                for cron in crons {
                    match cron.find_next_occurrence(&after, false) {
                        Ok(next) => {
                            earliest = Some(earliest.map_or(next.clone(), |best| best.min(next)))
                        }
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
            Self::Every(period) => SignedDuration::try_from(*period)
                .ok()
                .and_then(|period| after.checked_add(period).ok())
                .ok_or_else(|| anyhow!("The interval {period:?} is too long to schedule")),
        }
    }

    /// The schedule in words, one line per cron expression, for a person to check it says what they meant.
    pub fn describe(&self) -> Vec<String> {
        match self {
            Self::Cron(crons) => crons
                .iter()
                .map(|cron| format!("{}  {}", cron.as_str(), cron.describe()))
                .collect(),
            Self::Every(period) => vec![format!("Every {}", format_interval(*period))],
        }
    }

    /// How many runs fall on `day`, a calendar day in `zone`. Stops counting at a large number, so a tiny interval
    /// gives that number, not the true one.
    pub fn runs_on(&self, day: Date, zone: &TimeZone) -> anyhow::Result<usize> {
        let start =
            start_of_day(day, zone).with_context(|| format!("Can't tell when {day} starts"))?;
        let end = day
            .tomorrow()
            .map_err(anyhow::Error::from)
            .and_then(|next| start_of_day(next, zone))
            .with_context(|| format!("Can't tell when {day} ends"))?;
        let mut count = 0;
        // Strictly after, so a run at the very start of the day is counted.
        let mut at = start - SignedDuration::from_secs(1);
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
    pub fn upcoming(
        &self,
        after: ZonedDateTime,
        count: usize,
    ) -> anyhow::Result<Vec<ZonedDateTime>> {
        let mut at = after;
        (0..count)
            .map(|_| {
                at = self.next_after(at.clone())?;
                Ok(at.clone())
            })
            .collect()
    }
}

/// A length of time written as people write it: `90s`, `10m`, `1h 30m`, `2 days`. Months and years are not lengths
/// (they differ from one to the next), so they are not accepted. Nothing negative either.
pub fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let span: jiff::Span = text.trim().parse()?;
    let length = span.to_duration(jiff::SpanRelativeTo::days_are_24_hours())?;
    Duration::try_from(length).map_err(|_| anyhow!("An interval can't be negative"))
}

/// A length of time as `parse_interval` reads it: `1m 30s`.
pub fn format_interval(period: Duration) -> String {
    match jiff::SignedDuration::try_from(period) {
        Ok(length) => format!("{length:#}"),
        // Too long for a signed duration; the plain form is still true.
        Err(_) => format!("{period:?}"),
    }
}

/// Midnight at the start of `day`; in a zone where the clocks change at midnight, the first moment that exists.
fn start_of_day(day: Date, zone: &TimeZone) -> anyhow::Result<ZonedDateTime> {
    Ok(day.to_zoned_datetime(zone.clone())?)
}

#[cfg(test)]
mod tests {
    use crate::domain::services::clock::zone as named_zone;
    use crate::domain::services::clock::{local, london, utc};
    use jiff::Timestamp;

    use super::*;

    fn at(h: i8, m: i8, s: i8) -> ZonedDateTime {
        local(&london(), 2026, 6, 15, h, m, s)
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
        assert!(
            Schedule::Every(Duration::MAX)
                .next_after(at(8, 0, 0))
                .is_err()
        );
    }

    #[test]
    fn intervals_are_read_as_people_write_them() {
        let secs = |text| parse_interval(text).unwrap().as_secs();
        assert_eq!(secs("90s"), 90);
        assert_eq!(secs("10m"), 600);
        assert_eq!(secs("1h 30m"), 5400);
        assert_eq!(secs("2 days"), 2 * 24 * 3600);
        assert_eq!(parse_interval("1ms").unwrap(), Duration::from_millis(1));
        for bad in ["10", "-5m", "1y", "2mo", ""] {
            assert!(
                parse_interval(bad).is_err(),
                "{bad:?} should not be an interval"
            );
        }
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

    fn day(d: i8) -> Date {
        jiff::civil::date(2026, 6, d)
    }

    #[test]
    fn several_expressions_run_whichever_is_next() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        // Monday 2026-06-15.
        assert_eq!(schedule.next_after(at(6, 59, 0)).unwrap(), at(7, 0, 0));
        assert_eq!(
            schedule.next_after(at(7, 0, 0)).unwrap(),
            at(7, 1, 0),
            "every minute in the morning"
        );
        assert_eq!(schedule.next_after(at(8, 59, 30)).unwrap(), at(9, 0, 0));
        assert_eq!(
            schedule.next_after(at(9, 0, 0)).unwrap(),
            at(9, 5, 0),
            "every five minutes after it"
        );
        assert_eq!(schedule.next_after(at(21, 57, 0)).unwrap(), at(22, 0, 0));
        assert_eq!(
            schedule.next_after(at(22, 0, 0)).unwrap(),
            at(22, 15, 0),
            "every fifteen at night"
        );
    }

    #[test]
    fn the_week_has_the_runs_it_was_written_for() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        // 4 h a minute, 11 h every five minutes, 9 h every fifteen.
        assert_eq!(
            schedule.runs_on(day(15), &london()).unwrap(),
            240 + 132 + 36
        );
        assert_eq!(
            schedule.runs_on(day(19), &london()).unwrap(),
            240 + 132 + 36,
            "Friday"
        );
        // Hourly from 08:00 to 21:00, and nothing in the small hours or the evening.
        assert_eq!(
            schedule.runs_on(day(20), &london()).unwrap(),
            14,
            "Saturday"
        );
        assert_eq!(schedule.runs_on(day(21), &london()).unwrap(), 14, "Sunday");
    }

    #[test]
    fn runs_are_counted_for_the_other_kinds_too() {
        assert_eq!(
            Schedule::parse_cron("*/10 * * * *")
                .unwrap()
                .runs_on(day(15), &london())
                .unwrap(),
            144
        );
        assert_eq!(
            Schedule::parse_every("1h")
                .unwrap()
                .runs_on(day(15), &london())
                .unwrap(),
            24
        );
        // A tiny interval is counted up to a limit, not for ever.
        assert_eq!(
            Schedule::parse_every("1ms")
                .unwrap()
                .runs_on(day(15), &london())
                .unwrap(),
            MAX_RUNS_COUNTED
        );
    }

    #[test]
    fn the_next_runs_are_listed_in_order() {
        let schedule = Schedule::parse_crons(WEEK).unwrap();
        let runs = schedule.upcoming(at(8, 58, 0), 4).unwrap();
        assert_eq!(
            runs,
            vec![at(8, 59, 0), at(9, 0, 0), at(9, 5, 0), at(9, 10, 0)]
        );
    }

    #[test]
    fn a_schedule_is_described_in_words_one_line_each() {
        let lines = Schedule::parse_crons(["*/10 * * * *", "0 8-21 * * 6,0"])
            .unwrap()
            .describe();
        assert_eq!(
            lines,
            [
                "*/10 * * * *  At every 10 minutes.",
                "0 8-21 * * 6,0  At minute 0, of hour 8-21, on Sunday and Saturday.",
            ]
        );
        assert_eq!(
            Schedule::parse_every("90s").unwrap().describe(),
            ["Every 1m 30s"]
        );
    }

    #[test]
    fn a_list_needs_every_expression_to_be_valid_and_at_least_one() {
        let error = Schedule::parse_crons(["*/10 * * * *", "nonsense"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("nonsense"), "{error}");
        assert!(Schedule::parse_crons(Vec::<String>::new()).is_err());
    }

    #[test]
    fn an_expression_with_nothing_left_does_not_stop_the_others() {
        // 31 February never comes; the other still does.
        let schedule = Schedule::parse_crons(["0 0 31 2 *", "*/10 * * * *"]).unwrap();
        assert_eq!(schedule.next_after(at(8, 3, 0)).unwrap(), at(8, 10, 0));
        assert!(
            Schedule::parse_crons(["0 0 31 2 *"])
                .unwrap()
                .next_after(at(8, 3, 0))
                .is_err()
        );
    }

    // ---- Clock changes, in several zones ----

    fn offset_seconds(zone: &TimeZone, at: Timestamp) -> i32 {
        at.to_zoned_datetime(zone.clone()).offset().seconds()
    }

    /// The start of the minute at or after `at`: a cron expression has only minutes.
    fn ceil_to_minute(at: Timestamp) -> Timestamp {
        let seconds = at.as_second() + i64::from(at.subsec_nanosecond() > 0);
        Timestamp::from_second((seconds + 59).div_euclid(60) * 60).unwrap()
    }

    fn utc5(y: i16, m: i8, d: i8, h: i8, min: i8) -> Timestamp {
        utc(y, m, d, h, min, 0)
    }

    /// The instants in 2026 at which `zone`'s offset changes, to the minute, and by how many seconds.
    fn changes_in_2026(zone: &TimeZone) -> Vec<(Timestamp, i32)> {
        let mut found = Vec::new();
        let mut t = utc5(2026, 1, 1, 0, 0);
        while t < utc5(2027, 1, 1, 0, 0) {
            let next = t + SignedDuration::from_hours(1);
            let (a, b) = (offset_seconds(zone, t), offset_seconds(zone, next));
            if a != b {
                let mut minute = t;
                while offset_seconds(zone, minute) == a {
                    minute += SignedDuration::from_mins(1);
                }
                found.push((minute, b - a));
            }
            t = next;
        }
        found
    }

    /// What a run means: every minute, in UTC, whose local time matches. Slow and plainly right.
    fn reference(
        cron: &Cron,
        zone: &TimeZone,
        after: Timestamp,
        until: Timestamp,
    ) -> Vec<Timestamp> {
        let mut minute = ceil_to_minute(after);
        if minute <= after {
            minute += SignedDuration::from_mins(1);
        }
        let mut found = Vec::new();
        while minute <= until {
            if cron
                .is_time_matching(&minute.to_zoned_datetime(zone.clone()))
                .unwrap()
            {
                found.push(minute);
            }
            minute += SignedDuration::from_mins(1);
        }
        found
    }

    fn zones() -> [TimeZone; 8] {
        [
            london(),
            named_zone("America/New_York"),
            named_zone("Australia/Sydney"),
            named_zone("Australia/Lord_Howe"),
            named_zone("America/Santiago"),
            named_zone("Pacific/Chatham"),
            named_zone("Asia/Colombo"),
            named_zone("Asia/Kolkata"),
        ]
    }

    #[test]
    fn a_run_every_so_often_keeps_its_rhythm_through_every_clock_change_in_every_zone() {
        let mut checked_changes = 0;
        for zone in zones() {
            for expression in [
                "*/10 * * * *",
                "*/15 * * * *",
                "0 * * * *",
                "*/20 0-6 * * *",
                "7 * * * *",
            ] {
                let cron: Cron = expression.parse().unwrap();
                let schedule = Schedule::parse_cron(expression).unwrap();
                for (change, _) in changes_in_2026(&zone) {
                    checked_changes += 1;
                    let from = change - SignedDuration::from_hours(30);
                    let until = change + SignedDuration::from_hours(30);
                    let expected = reference(&cron, &zone, from, until);

                    let mut got = Vec::new();
                    let mut at = from.to_zoned_datetime(zone.clone());
                    loop {
                        let next = schedule.next_after(at.clone()).unwrap();
                        assert!(next > at, "{zone:?} {expression}: {next} is not after {at}");
                        if next.timestamp() > until {
                            break;
                        }
                        got.push(next.timestamp());
                        at = next;
                    }
                    assert_eq!(got, expected, "{zone:?} {expression} around {change}");
                }
            }
        }
        // The zones above have both kinds of change, so this did look at clocks going back as well as forward.
        assert_eq!(
            checked_changes,
            5 * 12,
            "six zones with two changes each, for five expressions"
        );
    }

    #[test]
    fn from_any_moment_the_next_run_is_the_first_match_after_it() {
        // Starting between slots, on them, and a second either side, around the UK's autumn change.
        let zone = london();
        let cron: Cron = "*/10 * * * *".parse().unwrap();
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let change = utc5(2026, 10, 25, 1, 0);
        for offset_seconds in (-4000..4000).step_by(37) {
            let start = change + SignedDuration::from_secs(offset_seconds);
            let expected = reference(&cron, &zone, start, start + SignedDuration::from_hours(2))[0];
            let got = schedule
                .next_after(start.to_zoned_datetime(zone.clone()))
                .unwrap()
                .timestamp();
            assert_eq!(got, expected, "from {start}");
        }
    }

    #[test]
    fn the_uk_autumn_change_has_no_gap() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = london();
        // 00:50 BST on 25 October 2026; the clocks go back at 02:00 BST to 01:00 GMT.
        let mut at = local(&zone, 2026, 10, 25, 0, 50, 0);
        let mut previous = at.clone();
        for _ in 0..16 {
            at = schedule.next_after(at).unwrap();
            assert_eq!(
                at.duration_since(&previous).as_secs() / 60,
                10,
                "{previous} to {at}"
            );
            previous = at.clone();
        }
        // It went through 01:50 BST and on into 01:00 GMT, an hour of real time later.
        assert_eq!(at.strftime("%H:%M %Z").to_string(), "02:30 GMT");
    }

    #[test]
    fn a_day_with_a_clock_change_has_the_runs_its_length_allows() {
        let runs = |expression: &str, zone: &TimeZone, y, m, d| {
            Schedule::parse_cron(expression)
                .unwrap()
                .runs_on(jiff::civil::date(y, m, d), zone)
                .unwrap()
        };
        // 25 hours in the UK on 25 October, 23 on 29 March.
        assert_eq!(runs("*/10 * * * *", &london(), 2026, 10, 25), 150);
        assert_eq!(runs("*/10 * * * *", &london(), 2026, 3, 29), 138);
        assert_eq!(runs("0 * * * *", &london(), 2026, 10, 25), 25);
        assert_eq!(runs("0 * * * *", &london(), 2026, 3, 29), 23);
        assert_eq!(runs("*/10 * * * *", &london(), 2026, 10, 24), 144);
        // The southern hemisphere changes the other way round: Sydney goes back on 5 April, forward on 4 October.
        assert_eq!(
            runs("*/10 * * * *", &named_zone("Australia/Sydney"), 2026, 4, 5),
            150
        );
        assert_eq!(
            runs("*/10 * * * *", &named_zone("Australia/Sydney"), 2026, 10, 4),
            138
        );
        // Lord Howe changes by half an hour.
        assert_eq!(
            runs(
                "*/10 * * * *",
                &named_zone("Australia/Lord_Howe"),
                2026,
                4,
                5
            ),
            147
        );
        assert_eq!(
            runs(
                "*/10 * * * *",
                &named_zone("Australia/Lord_Howe"),
                2026,
                10,
                4
            ),
            141
        );
        // Colombo and Kolkata have no change, and an offset of a half hour: every day is the same.
        for zone in [named_zone("Asia/Colombo"), named_zone("Asia/Kolkata")] {
            for (m, d) in [(3, 29), (4, 5), (10, 4), (10, 25)] {
                assert_eq!(
                    runs("*/10 * * * *", &zone, 2026, m, d),
                    144,
                    "{zone:?} {m}-{d}"
                );
            }
        }
    }

    /// The convention of Vixie cron, and what `croner` does: a job at a fixed time runs once even when that time is
    /// on the clock twice, while a job that is "every so often" keeps its rhythm through the repeated hour (the
    /// brute-force test above). A fixed time that doesn't exist, in the hour that is skipped, runs at the change.
    #[test]
    fn a_fixed_time_runs_once_in_a_repeated_hour_and_one_that_was_skipped_runs_at_the_change() {
        let zone = london();
        let daily = Schedule::parse_cron("30 1 * * *").unwrap();
        // Autumn: 01:30 is on the clock twice, and the daily run happens once, the first time.
        let first = daily
            .next_after(local(&zone, 2026, 10, 25, 0, 0, 0))
            .unwrap();
        assert_eq!(first.strftime("%d %H:%M %Z").to_string(), "25 01:30 BST");
        assert_eq!(
            daily
                .next_after(first.clone())
                .unwrap()
                .strftime("%d %H:%M %Z")
                .to_string(),
            "26 01:30 GMT"
        );
        assert_eq!(
            Schedule::parse_cron("30 1 * * *")
                .unwrap()
                .runs_on(jiff::civil::date(2026, 10, 25), &zone)
                .unwrap(),
            1
        );
        // Spring: 01:30 doesn't exist, and the run happens when the clocks change.
        let spring = daily
            .next_after(local(&zone, 2026, 3, 29, 0, 0, 0))
            .unwrap();
        assert_eq!(spring.timestamp(), utc5(2026, 3, 29, 1, 0));
        // And the day after is the normal one.
        assert_eq!(
            daily
                .next_after(spring.clone())
                .unwrap()
                .strftime("%d %H:%M")
                .to_string(),
            "30 01:30"
        );
    }

    #[test]
    fn a_result_is_never_the_moment_it_was_asked_from() {
        // Where the clocks go forward, the answer used to name the instant it started from.
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = london();
        let mut at = utc5(2026, 3, 29, 0, 30).to_zoned_datetime(zone.clone());
        for _ in 0..12 {
            let next = schedule.next_after(at.clone()).unwrap();
            assert!(next > at, "{at} then {next}");
            at = next;
        }
    }

    #[test]
    fn a_span_with_no_change_in_it_costs_nothing_extra_and_is_unchanged() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let zone = named_zone("Asia/Colombo");
        let at = local(&zone, 2026, 6, 15, 8, 3, 20);
        assert_eq!(
            schedule.next_after(at).unwrap(),
            local(&zone, 2026, 6, 15, 8, 10, 0)
        );
    }

    #[test]
    fn several_expressions_take_the_earliest_across_a_repeated_hour() {
        let schedule = Schedule::parse_crons(["0 1 * * *", "*/30 * * * *"]).unwrap();
        let zone = london();
        let mut at = local(&zone, 2026, 10, 25, 0, 40, 0);
        let mut gaps = Vec::new();
        for _ in 0..5 {
            let next = schedule.next_after(at.clone()).unwrap();
            gaps.push(next.duration_since(&at).as_secs() / 60);
            at = next;
        }
        // 01:00 BST, 01:30 BST, 01:00 GMT, 01:30 GMT, 02:00 GMT.
        assert_eq!(gaps, [20, 30, 30, 30, 30]);
    }
}

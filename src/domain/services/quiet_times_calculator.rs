use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

use crate::domain::models::train::TrainPassage;

#[derive(Debug, Clone, Copy)]
pub struct TimeWindow {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct QuietTimesRules {
    pub buffer: Duration,
    pub weekday_window: TimeWindow,
    pub weekend_window: TimeWindow,
    /// When false, gaps are computed across the whole day (00:00-23:59:59)
    /// instead of being restricted to `weekday_window`/`weekend_window`.
    pub use_time_windows: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

impl Gap {
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }
}

#[derive(Debug, Clone)]
pub struct DailyReport {
    pub date: NaiveDate,
    pub window_start: NaiveDateTime,
    pub window_end: NaiveDateTime,
    pub train_count: usize,
    pub gaps: Vec<Gap>,
}

impl DailyReport {
    /// The single longest gap on this day, if there were any.
    pub fn largest_gap(&self) -> Option<&Gap> {
        self.gaps.iter().max_by_key(|gap| gap.duration())
    }

    /// This day's own gaps, longest first, capped at `limit`.
    pub fn longest_gaps(&self, limit: usize) -> Vec<&Gap> {
        let mut gaps: Vec<&Gap> = self.gaps.iter().collect();
        gaps.sort_by(|a, b| b.duration().cmp(&a.duration()));
        gaps.truncate(limit);
        gaps
    }
}

#[derive(Debug, Clone, derive_new::new)]
pub struct QuietTimesReport {
    pub days: Vec<DailyReport>,
}

impl QuietTimesReport {
    /// Every gap across the whole range, longest first.
    pub fn longest_gaps(&self, limit: usize) -> Vec<(NaiveDate, Gap)> {
        let mut all: Vec<(NaiveDate, Gap)> = self
            .days
            .iter()
            .flat_map(|day| day.gaps.iter().cloned().map(move |gap| (day.date, gap)))
            .collect();
        all.sort_by(|a, b| b.1.duration().cmp(&a.1.duration()));
        all.truncate(limit);
        all
    }
}

pub struct QuietTimesCalculator {
    rules: QuietTimesRules,
}

impl QuietTimesCalculator {
    pub fn new(rules: QuietTimesRules) -> Self {
        Self { rules }
    }

    /// `passages` must already include both `date`'s and the previous calendar
    /// date's resolved passages: a train counted against the previous day's
    /// schedule can physically pass after midnight, landing in `date`'s
    /// early-morning window.
    pub fn report_for_day(&self, date: NaiveDate, passages: &[TrainPassage]) -> DailyReport {
        let window = if !self.rules.use_time_windows {
            TimeWindow {
                start: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
                end: NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
            }
        } else if date.weekday().num_days_from_monday() >= 5 {
            self.rules.weekend_window
        } else {
            self.rules.weekday_window
        };
        let window_start = date.and_time(window.start);
        let window_end = date.and_time(window.end);

        let buffer = self.rules.buffer;
        let mut times: Vec<NaiveDateTime> = passages
            .iter()
            .map(|p| p.scheduled_at)
            .filter(|&t| t >= window_start - buffer && t <= window_end + buffer)
            .collect();
        times.sort();

        // Every gap between (buffered) passages is kept, however small; it's up to
        // whoever presents this report to decide which gaps are worth listing.
        let mut gaps = Vec::new();
        let mut cursor = window_start;
        for &t in &times {
            if t - buffer > cursor {
                gaps.push(Gap {
                    start: cursor,
                    end: t - buffer,
                });
            }
            cursor = cursor.max(t + buffer);
        }
        if window_end > cursor {
            gaps.push(Gap {
                start: cursor,
                end: window_end,
            });
        }

        DailyReport {
            date,
            window_start,
            window_end,
            train_count: times.len(),
            gaps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speculoos::prelude::*;

    fn rules(buffer_min: i64) -> QuietTimesRules {
        rules_with_windows(buffer_min, true)
    }

    fn rules_with_windows(buffer_min: i64, use_time_windows: bool) -> QuietTimesRules {
        QuietTimesRules::new(
            Duration::minutes(buffer_min),
            TimeWindow {
                start: NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
                end: NaiveTime::from_hms_opt(23, 59, 0).unwrap(),
            },
            TimeWindow {
                start: NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
                end: NaiveTime::from_hms_opt(23, 59, 0).unwrap(),
            },
            use_time_windows,
        )
    }

    fn passage_at(date: NaiveDate, hh: u32, mm: u32) -> TrainPassage {
        TrainPassage::new(
            "UID1".to_owned(),
            date.and_hms_opt(hh, mm, 0).unwrap(),
        )
    }

    fn wed() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 10).unwrap() // a Wednesday
    }

    fn sat() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 13).unwrap() // a Saturday
    }

    #[test]
    fn every_gap_between_passages_is_kept_however_small() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();
        // Two trains 12 minutes apart -> a short 8 minute buffered gap.
        let passages = vec![passage_at(date, 18, 0), passage_at(date, 18, 12)];

        let report = calculator.report_for_day(date, &passages);

        let gap = report
            .gaps
            .iter()
            .find(|g| g.start == date.and_hms_opt(18, 2, 0).unwrap())
            .expect("expected a buffered gap starting at 18:02, however small");
        assert_that!(gap.end).is_equal_to(date.and_hms_opt(18, 10, 0).unwrap());
    }

    #[test]
    fn overlapping_buffers_produce_no_gap_between_passages() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();
        // 3 minutes apart with a 2 minute buffer either side -> buffers overlap, no gap between them.
        let passages = vec![passage_at(date, 18, 0), passage_at(date, 18, 3)];

        let report = calculator.report_for_day(date, &passages);

        let between_the_two_trains = report.gaps.iter().find(|g| {
            g.start >= date.and_hms_opt(17, 58, 0).unwrap()
                && g.end <= date.and_hms_opt(18, 5, 0).unwrap()
        });
        assert_that!(between_the_two_trains).is_none();
    }

    #[test]
    fn weekend_uses_weekend_window() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = sat();

        let report = calculator.report_for_day(date, &[]);

        assert_that!(report.window_start).is_equal_to(date.and_hms_opt(7, 0, 0).unwrap());
        assert_that!(report.window_end).is_equal_to(date.and_hms_opt(23, 59, 0).unwrap());
    }

    #[test]
    fn weekday_uses_weekday_window() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();

        let report = calculator.report_for_day(date, &[]);

        assert_that!(report.window_start).is_equal_to(date.and_hms_opt(17, 0, 0).unwrap());
    }

    #[test]
    fn empty_day_reports_a_single_gap_spanning_the_whole_window() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();

        let report = calculator.report_for_day(date, &[]);

        assert_that!(report.train_count).is_equal_to(0);
        assert_that!(report.gaps).has_length(1);
        assert_that!(report.gaps[0].start).is_equal_to(report.window_start);
        assert_that!(report.gaps[0].end).is_equal_to(report.window_end);
    }

    #[test]
    fn post_midnight_passage_from_previous_day_falls_inside_todays_window() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();
        // A passage timestamped just after midnight on `date`, as produced when
        // combining `date`'s and the previous day's resolved passages.
        let passages = vec![passage_at(date, 0, 30)];

        let report = calculator.report_for_day(date, &passages);

        // 00:30 is outside the 17:00-23:59 weekday window, so it should not be counted.
        assert_that!(report.train_count).is_equal_to(0);
    }

    #[test]
    fn disabling_time_windows_uses_the_whole_day() {
        let calculator = QuietTimesCalculator::new(rules_with_windows(2, false));
        let date = wed();

        let report = calculator.report_for_day(date, &[]);

        assert_that!(report.window_start).is_equal_to(date.and_hms_opt(0, 0, 0).unwrap());
        assert_that!(report.window_end).is_equal_to(date.and_hms_opt(23, 59, 59).unwrap());
    }

    #[test]
    fn disabling_time_windows_counts_early_morning_passages() {
        let calculator = QuietTimesCalculator::new(rules_with_windows(2, false));
        let date = wed();
        // 00:30 is outside the weekday window but should count once windows are disabled.
        let passages = vec![passage_at(date, 0, 30)];

        let report = calculator.report_for_day(date, &passages);

        assert_that!(report.train_count).is_equal_to(1);
    }

    #[test]
    fn largest_gap_returns_the_longest_gap_on_the_day() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();
        // Trains at 18:00 and 19:00 -> a short gap 18:02-18:58, then a longer one 19:02-23:59.
        let passages = vec![passage_at(date, 18, 0), passage_at(date, 19, 0)];

        let report = calculator.report_for_day(date, &passages);
        let largest = report.largest_gap().expect("expected at least one gap");

        assert_that!(largest.start).is_equal_to(date.and_hms_opt(19, 2, 0).unwrap());
        assert_that!(largest.end).is_equal_to(report.window_end);
    }

    #[test]
    fn daily_report_longest_gaps_sorts_descending_and_truncates() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date = wed();
        // Trains at 18:00 and 19:00 -> a short gap 18:02-18:58 (56 min), then a longer one 19:02-23:59.
        let passages = vec![passage_at(date, 18, 0), passage_at(date, 19, 0)];

        let report = calculator.report_for_day(date, &passages);
        let top = report.longest_gaps(1);

        assert_that!(top).has_length(1);
        assert_that!(top[0].start).is_equal_to(date.and_hms_opt(19, 2, 0).unwrap());
    }

    #[test]
    fn longest_gaps_sorts_descending_and_truncates() {
        let calculator = QuietTimesCalculator::new(rules(2));
        let date1 = wed();
        let date2 = sat();

        let report1 = calculator.report_for_day(date1, &[passage_at(date1, 18, 0)]);
        let report2 = calculator.report_for_day(date2, &[passage_at(date2, 18, 0)]);
        let report = QuietTimesReport::new(vec![report1, report2]);

        let longest = report.longest_gaps(1);

        assert_that!(longest).has_length(1);
        // Saturday's window starts earlier (07:00) so its pre-train gap is longer.
        assert_that!(longest[0].0).is_equal_to(date2);
    }
}

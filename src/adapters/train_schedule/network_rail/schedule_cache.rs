use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::adapters::train_schedule::network_rail::response::{JsonScheduleV1, RecordEnvelope};
use crate::domain::models::train::TrainPassage;

/// Non-rail replacement services (buses, ships) that never physically pass a timing point.
const NON_RAIL_STATUS: [&str; 4] = ["B", "5", "S", "4"];

/// Which schedule wins on a given date when more than one covers it: lower rank wins.
fn stp_rank(stp: &str) -> u8 {
    match stp {
        "C" => 0,
        "N" => 1,
        "O" => 2,
        "P" => 3,
        _ => 9,
    }
}

/// Parses a CIF "HHMM" or "HHMMH" time into minutes since midnight; the
/// trailing `H` marks a half-minute (30 second) offset.
pub fn parse_hhmm(raw: Option<&str>) -> Option<f64> {
    let s = raw?;
    if s.len() < 4 {
        return None;
    }
    let hours: f64 = s[0..2].parse().ok()?;
    let minutes: f64 = s[2..4].parse().ok()?;
    let half = if s.ends_with('H') { 0.5 } else { 0.0 };
    Some(hours * 60.0 + minutes + half)
}

/// A schedule filtered down to just what's needed to resolve passages at the
/// configured timing points, kept between `--build` runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedScheduleRecord {
    pub uid: String,
    pub start_date: String,
    pub end_date: String,
    pub days_runs: String,
    pub stp_indicator: String,
    pub minutes_past_midnight: Option<f64>,
}

/// Filters raw CIF SCHEDULE NDJSON lines down to records relevant to `watch_tiplocs`,
/// resolving each schedule's pass time at the highest-priority matching timing point.
pub fn build_cache_records<I>(
    lines: I,
    watch_tiplocs: &[String],
) -> anyhow::Result<Vec<CachedScheduleRecord>>
where
    I: IntoIterator<Item = String>,
{
    let mut records = Vec::new();
    for line in lines {
        let Ok(envelope) = serde_json::from_str::<RecordEnvelope>(&line) else {
            continue;
        };
        let Some(schedule) = envelope.schedule else {
            continue;
        };
        if schedule.transaction_type.as_deref() == Some("Delete") {
            continue;
        }
        if let Some(record) = resolve_record(&schedule, watch_tiplocs) {
            records.push(record);
        }
    }
    Ok(records)
}

fn resolve_record(
    schedule: &JsonScheduleV1,
    watch_tiplocs: &[String],
) -> Option<CachedScheduleRecord> {
    let stp = schedule.stp_indicator.clone();
    let is_non_rail = schedule
        .train_status
        .as_deref()
        .is_some_and(|status| NON_RAIL_STATUS.contains(&status));

    let mut minutes = None;
    if !is_non_rail {
        if let Some(locs) = schedule
            .schedule_segment
            .as_ref()
            .and_then(|s| s.schedule_location.as_ref())
            .filter(|locs| !locs.is_empty())
        {
            let origin = parse_hhmm(locs[0].departure.as_deref());
            'outer: for code in watch_tiplocs {
                for loc in locs {
                    if loc.tiploc_code.as_deref() != Some(code.as_str()) {
                        continue;
                    }
                    let Some(mut t) = parse_hhmm(loc.pass.as_deref())
                        .or_else(|| parse_hhmm(loc.departure.as_deref()))
                        .or_else(|| parse_hhmm(loc.arrival.as_deref()))
                    else {
                        continue;
                    };
                    if let Some(origin) = origin {
                        if t < origin {
                            t += 1440.0; // passes after midnight
                        }
                    }
                    minutes = Some(t);
                    break 'outer;
                }
            }
        }
    }

    // A permanent schedule that never passes any watched timing point is of no interest;
    // overlays/new/cancelled schedules are kept regardless, since they can override a
    // permanent schedule that DOES pass (see `passages_on_date`'s STP overlay resolution).
    if minutes.is_none() && stp == "P" {
        return None;
    }

    Some(CachedScheduleRecord {
        uid: schedule.train_uid.clone(),
        start_date: schedule.schedule_start_date.clone(),
        end_date: schedule.schedule_end_date.clone(),
        days_runs: schedule.schedule_days_runs.clone(),
        stp_indicator: stp,
        minutes_past_midnight: minutes,
    })
}

/// Resolves, per train UID, which cached schedule applies on `date` (STP overlay rank +
/// day-of-week bitmask + validity range), returning already-adjusted passage timestamps.
pub fn passages_on_date(records: &[CachedScheduleRecord], date: NaiveDate) -> Vec<TrainPassage> {
    let interest: HashSet<&str> = records
        .iter()
        .filter(|r| r.minutes_past_midnight.is_some())
        .map(|r| r.uid.as_str())
        .collect();

    let mut by_uid: HashMap<&str, Vec<&CachedScheduleRecord>> = HashMap::new();
    for record in records {
        if interest.contains(record.uid.as_str()) {
            by_uid.entry(record.uid.as_str()).or_default().push(record);
        }
    }

    let date_str = date.format("%Y-%m-%d").to_string();
    let day_of_week = date.weekday().num_days_from_monday() as usize;
    let midnight = date.and_hms_opt(0, 0, 0).unwrap();

    let mut out = Vec::new();
    for (uid, recs) in by_uid {
        let valid: Vec<&CachedScheduleRecord> = recs
            .into_iter()
            .filter(|r| {
                r.start_date.as_str() <= date_str.as_str()
                    && date_str.as_str() <= r.end_date.as_str()
                    && r.days_runs.as_bytes().get(day_of_week) == Some(&b'1')
            })
            .collect();

        let Some(best) = valid.into_iter().min_by_key(|r| stp_rank(&r.stp_indicator)) else {
            continue;
        };
        if best.stp_indicator == "C" {
            continue;
        }
        if let Some(minutes) = best.minutes_past_midnight {
            let total_seconds = (minutes * 60.0).round() as i64;
            out.push(TrainPassage::new(
                uid.to_owned(),
                midnight + chrono::Duration::seconds(total_seconds),
            ));
        }
    }
    out
}

pub fn load_cache(path: &Path) -> anyhow::Result<Vec<CachedScheduleRecord>> {
    let data = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}

pub fn save_cache(path: &Path, records: &[CachedScheduleRecord]) -> anyhow::Result<()> {
    let data = serde_json::to_string(records)?;
    std::fs::write(path, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use speculoos::prelude::*;

    #[test]
    fn parses_plain_hhmm() {
        assert_that!(parse_hhmm(Some("1730"))).is_equal_to(Some(1050.0));
    }

    #[test]
    fn parses_half_minute_suffix() {
        assert_that!(parse_hhmm(Some("1730H"))).is_equal_to(Some(1050.5));
    }

    #[test]
    fn parses_missing_time_as_none() {
        assert_that!(parse_hhmm(None)).is_equal_to(None);
        assert_that!(parse_hhmm(Some(""))).is_equal_to(None);
    }

    #[test]
    fn stp_rank_orders_cancelled_first_and_permanent_last() {
        assert_that!(stp_rank("C")).is_less_than(stp_rank("N"));
        assert_that!(stp_rank("N")).is_less_than(stp_rank("O"));
        assert_that!(stp_rank("O")).is_less_than(stp_rank("P"));
    }

    fn record(
        uid: &str,
        start: &str,
        end: &str,
        days_runs: &str,
        stp: &str,
        minutes: Option<f64>,
    ) -> CachedScheduleRecord {
        CachedScheduleRecord {
            uid: uid.to_owned(),
            start_date: start.to_owned(),
            end_date: end.to_owned(),
            days_runs: days_runs.to_owned(),
            stp_indicator: stp.to_owned(),
            minutes_past_midnight: minutes,
        }
    }

    fn wednesday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 10).unwrap()
    }

    #[test]
    fn overlay_schedule_beats_permanent_schedule() {
        let records = vec![
            record("UID1", "2024-01-01", "2024-12-31", "1111111", "P", Some(1000.0)),
            record("UID1", "2024-01-10", "2024-01-10", "1111111", "O", Some(1100.0)),
        ];

        let passages = passages_on_date(&records, wednesday());

        assert_that!(passages).has_length(1);
        assert_that!(passages[0].scheduled_at)
            .is_equal_to(wednesday().and_hms_opt(0, 0, 0).unwrap() + chrono::Duration::minutes(1100));
    }

    #[test]
    fn cancelled_schedule_is_excluded_even_when_best_ranked() {
        let records = vec![record(
            "UID1",
            "2024-01-01",
            "2024-12-31",
            "1111111",
            "C",
            Some(1000.0),
        )];

        assert_that!(passages_on_date(&records, wednesday())).is_empty();
    }

    #[test]
    fn day_bitmask_mismatch_excludes_record() {
        // Sunday-only schedule, queried on a Wednesday.
        let records = vec![record(
            "UID1",
            "2024-01-01",
            "2024-12-31",
            "0000001",
            "P",
            Some(1000.0),
        )];

        assert_that!(passages_on_date(&records, wednesday())).is_empty();
    }

    #[test]
    fn date_range_boundaries_are_inclusive() {
        let records = vec![record(
            "UID1",
            "2024-01-10",
            "2024-01-10",
            "1111111",
            "P",
            Some(1000.0),
        )];

        assert_that!(passages_on_date(&records, wednesday())).has_length(1);
    }

    #[test]
    fn build_cache_records_skips_non_schedule_and_delete_lines() {
        let lines = vec![
            r#"{"TiplocV1": {"tiploc_code": "HRNSY", "tps_description": "HORNSEY"}}"#.to_owned(),
            r#"{"JsonScheduleV1": {"transaction_type": "Delete", "CIF_train_uid": "X1", "schedule_start_date": "2024-01-01", "schedule_end_date": "2024-01-01", "schedule_days_runs": "1111111", "CIF_stp_indicator": "P"}}"#.to_owned(),
        ];

        let records = build_cache_records(lines, &["HRNSY".to_owned()]).unwrap();

        assert_that!(records).is_empty();
    }

    #[test]
    fn build_cache_records_drops_permanent_schedule_with_no_resolved_minutes() {
        let lines = vec![
            r#"{"JsonScheduleV1": {"CIF_train_uid": "X1", "schedule_start_date": "2024-01-01", "schedule_end_date": "2024-01-01", "schedule_days_runs": "1111111", "CIF_stp_indicator": "P", "schedule_segment": {"schedule_location": [{"tiploc_code": "OTHER", "departure": "1000"}]}}}"#.to_owned(),
        ];

        let records = build_cache_records(lines, &["HRNSY".to_owned()]).unwrap();

        assert_that!(records).is_empty();
    }

    #[test]
    fn build_cache_records_resolves_pass_time_at_watched_timing_point() {
        let lines = vec![
            r#"{"JsonScheduleV1": {"CIF_train_uid": "X1", "schedule_start_date": "2024-01-01", "schedule_end_date": "2024-01-01", "schedule_days_runs": "1111111", "CIF_stp_indicator": "P", "schedule_segment": {"schedule_location": [{"tiploc_code": "ORIGIN", "departure": "1000"}, {"tiploc_code": "HRNSY", "pass": "1015"}]}}}"#.to_owned(),
        ];

        let records = build_cache_records(lines, &["HRNSY".to_owned()]).unwrap();

        assert_that!(records).has_length(1);
        assert_that!(records[0].minutes_past_midnight).is_equal_to(Some(615.0));
    }
}

use chrono::{DateTime, Local, NaiveTime, Timelike};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DepartureStatus {
    OnTime,
    Delayed,
    Cancelled,
    /// A live prediction with no timetable to compare against, e.g. TfL.
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, derive_new::new, Serialize)]
pub struct DepartureService {
    /// Clock time (HH:MM): the scheduled time, or the predicted one for a `Live` service.
    pub time: String,
    pub destination: String,
    pub status: DepartureStatus,
    /// The new time when `Delayed`, "late" if no estimate was given; otherwise empty.
    pub expected: String,
    /// How long until it leaves, e.g. "due" or "6 min"; empty when unknown or cancelled.
    pub countdown: String,
}

/// What a departures service returns for its stop.
#[derive(Debug, Clone, PartialEq, Eq, derive_new::new)]
pub struct Departures {
    /// The station or stop's own name, e.g. "Hornsey"; empty if the source didn't give one.
    pub station: String,
    pub services: Vec<DepartureService>,
}

/// Seconds from `now` until the next occurrence of `clock_time` ("HH:MM", local time).
/// Resolution is a minute, so it agrees with the clock on the display.
pub fn seconds_until(now: DateTime<Local>, clock_time: &str) -> Option<i64> {
    let time = NaiveTime::parse_from_str(clock_time, "%H:%M").ok()?;
    let minutes = |t: NaiveTime| i64::from(t.hour() * 60 + t.minute());
    let mut diff = minutes(time) - minutes(now.time());
    // Boards run across midnight: 00:10 at 23:55 is 15 minutes away, not -23h45.
    if diff < -12 * 60 {
        diff += 24 * 60;
    }
    Some(diff * 60)
}

/// "due" or "N min" for the time left; empty if it has already gone.
pub fn countdown(seconds: i64) -> String {
    match seconds {
        ..=-1 => String::new(),
        0..=59 => "due".to_owned(),
        s => format!("{} min", s / 60),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2024, 1, 10, hour, minute, second).unwrap()
    }

    #[test]
    fn counts_whole_clock_minutes_ignoring_seconds() {
        assert_eq!(seconds_until(at(9, 3, 40), "09:09"), Some(6 * 60));
    }

    #[test]
    fn wraps_past_midnight() {
        assert_eq!(seconds_until(at(23, 55, 0), "00:10"), Some(15 * 60));
    }

    #[test]
    fn a_time_just_gone_is_negative() {
        assert_eq!(seconds_until(at(9, 10, 0), "09:08"), Some(-2 * 60));
    }

    #[test]
    fn rejects_text_that_is_not_a_time() {
        assert_eq!(seconds_until(at(9, 0, 0), "Delayed"), None);
    }

    #[test]
    fn countdown_wording() {
        assert_eq!(countdown(-60), "");
        assert_eq!(countdown(0), "due");
        assert_eq!(countdown(59), "due");
        assert_eq!(countdown(60), "1 min");
        assert_eq!(countdown(6 * 60 + 20), "6 min");
    }
}

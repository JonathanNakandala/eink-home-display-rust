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

impl DepartureService {
    /// When it leaves, as a clock time: the new time for a delayed service that has one.
    fn leaves_at(&self) -> &str {
        match self.status {
            DepartureStatus::Delayed if NaiveTime::parse_from_str(&self.expected, "%H:%M").is_ok() => &self.expected,
            _ => &self.time,
        }
    }

    /// This service as it reads at `now`, for departures fetched a while ago: the countdown is
    /// recalculated, and None if it has already gone.
    pub fn as_of(&self, now: DateTime<Local>) -> Option<Self> {
        let seconds = seconds_until(now, self.leaves_at())?;
        if seconds < 0 {
            return None;
        }
        let mut service = self.clone();
        // A cancelled train, or a late one with no estimate, never had a countdown.
        if !service.countdown.is_empty() {
            service.countdown = countdown(seconds);
        }
        Some(service)
    }
}

/// What a departures service returns for its stop.
#[derive(Debug, Clone, PartialEq, Eq, derive_new::new)]
pub struct Departures {
    /// The station or stop's own name, e.g. "Hornsey"; empty if the source didn't give one.
    pub station: String,
    pub services: Vec<DepartureService>,
}

impl Departures {
    /// These departures as they read at `now`, without the services that have left.
    pub fn as_of(&self, now: DateTime<Local>) -> Self {
        Self {
            station: self.station.clone(),
            services: self.services.iter().filter_map(|service| service.as_of(now)).collect(),
        }
    }
}

/// Seconds from `now` until `clock_time` ("HH:MM", local time), negative if it has gone: whichever of
/// today, yesterday or tomorrow puts it nearest to now, so within 12 hours either way.
/// Resolution is a minute, so it agrees with the clock on the display.
pub fn seconds_until(now: DateTime<Local>, clock_time: &str) -> Option<i64> {
    let time = NaiveTime::parse_from_str(clock_time, "%H:%M").ok()?;
    let minutes = |t: NaiveTime| i64::from(t.hour() * 60 + t.minute());
    // Boards run across midnight, both ways: 00:10 at 23:55 is 15 minutes away, not -23h45, and 23:55 at
    // 00:05 (a train from the cached data of a board that has since been unreachable) left 10 minutes
    // ago, not 23h50 from now.
    let diff = (minutes(time) - minutes(now.time()) + 12 * 60).rem_euclid(24 * 60) - 12 * 60;
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
    fn a_time_before_midnight_has_gone_after_it() {
        assert_eq!(seconds_until(at(0, 5, 0), "23:55"), Some(-10 * 60));
        assert_eq!(seconds_until(at(0, 0, 0), "23:59"), Some(-60));
    }

    #[test]
    fn the_nearest_day_is_chosen_at_the_edges() {
        assert_eq!(seconds_until(at(12, 0, 0), "12:00"), Some(0));
        // Just under twelve hours ahead stays ahead; twelve hours or more behind is the next day.
        assert_eq!(seconds_until(at(12, 0, 0), "23:59"), Some(11 * 3600 + 59 * 60));
        assert_eq!(seconds_until(at(12, 0, 0), "00:00"), Some(-12 * 3600));
    }

    #[test]
    fn a_time_just_gone_is_negative() {
        assert_eq!(seconds_until(at(9, 10, 0), "09:08"), Some(-2 * 60));
    }

    #[test]
    fn rejects_text_that_is_not_a_time() {
        assert_eq!(seconds_until(at(9, 0, 0), "Delayed"), None);
    }

    fn service(time: &str, status: DepartureStatus, expected: &str, countdown: &str) -> DepartureService {
        DepartureService::new(time.into(), "X".into(), status, expected.into(), countdown.into())
    }

    #[test]
    fn old_departures_get_a_new_countdown_and_lose_the_ones_that_left() {
        let departures = Departures::new(
            "Hornsey".into(),
            vec![
                service("09:00", DepartureStatus::OnTime, "", "due"),
                service("09:10", DepartureStatus::OnTime, "", "10 min"),
                service("09:20", DepartureStatus::Delayed, "09:27", "27 min"),
            ],
        );

        let later = departures.as_of(at(9, 5, 0));
        assert_eq!(later.station, "Hornsey");
        let countdowns: Vec<_> = later.services.iter().map(|s| s.countdown.as_str()).collect();
        // 09:00 has gone; the delayed one counts to its new time.
        assert_eq!(later.services.len(), 2);
        assert_eq!(countdowns, ["5 min", "22 min"]);
        assert_eq!(later.services[1].expected, "09:27");
    }

    #[test]
    fn across_midnight_a_gone_service_is_dropped_and_an_upcoming_one_counts_down() {
        let departures = Departures::new(
            "Hornsey".into(),
            vec![
                service("23:55", DepartureStatus::OnTime, "", "10 min"),
                service("00:10", DepartureStatus::OnTime, "", "25 min"),
            ],
        );

        // Fetched at 23:45 and shown at 00:05, because the source has been down since.
        let later = departures.as_of(at(0, 5, 0));
        let countdowns: Vec<_> = later.services.iter().map(|s| s.countdown.as_str()).collect();
        assert_eq!(countdowns, ["5 min"], "the 23:55 left ten minutes ago");
    }

    #[test]
    fn cancelled_and_estimate_less_services_keep_no_countdown_until_they_go() {
        let cancelled = service("09:10", DepartureStatus::Cancelled, "", "");
        assert_eq!(cancelled.as_of(at(9, 5, 0)).unwrap().countdown, "");
        assert!(cancelled.as_of(at(9, 11, 0)).is_none());

        let late = service("09:10", DepartureStatus::Delayed, "late", "");
        assert_eq!(late.as_of(at(9, 5, 0)).unwrap().countdown, "");
    }

    #[test]
    fn a_service_leaving_this_minute_is_still_shown_as_due() {
        let now = service("09:05", DepartureStatus::OnTime, "", "5 min");
        assert_eq!(now.as_of(at(9, 5, 40)).unwrap().countdown, "due");
        assert!(now.as_of(at(9, 6, 0)).is_none());
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

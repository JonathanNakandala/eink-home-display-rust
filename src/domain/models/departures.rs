use chrono::{DateTime, Duration, LocalResult, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
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
            DepartureStatus::Delayed
                if NaiveTime::parse_from_str(&self.expected, "%H:%M").is_ok() =>
            {
                &self.expected
            }
            _ => &self.time,
        }
    }

    /// This service as it reads at `now`, for departures fetched a while ago: the countdown is
    /// recalculated, and None if it has already gone.
    pub fn as_of(&self, now: DateTime<Tz>) -> Option<Self> {
        let seconds = seconds_until(now, self.leaves_at(), now.timezone())?;
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
    pub fn as_of(&self, now: DateTime<Tz>) -> Self {
        Self {
            station: self.station.clone(),
            services: self
                .services
                .iter()
                .filter_map(|service| service.as_of(now))
                .collect(),
        }
    }
}

/// The moment a clock time ("HH:MM") means, near `now`, when it is read on the clock of `clock_zone`: whichever of
/// yesterday, today or tomorrow there puts it nearest to now (so within 12 hours either way, a tie going to the
/// past). A board gives only a time with no date, and it runs across midnight both ways: 00:10 at 23:55 is 15
/// minutes away, not 23h45 ago, and 23:55 at 00:05 (cached data from a board that has since been unreachable) left
/// 10 minutes ago.
///
/// Reading the time on `clock_zone`'s clock, rather than comparing it with the display's, is what keeps it right
/// where the two differ: a service that reports UK time, shown in another country, and the hour around a clock
/// change, where a clock time can be on the clock twice or not at all. A time that is twice on the clock is the
/// pass nearer to now.
pub fn clock_time_near(now: DateTime<Tz>, clock_time: &str, clock_zone: Tz) -> Option<DateTime<Utc>> {
    let time = NaiveTime::parse_from_str(clock_time, "%H:%M").ok()?;
    let now = now.with_timezone(&Utc);
    let today = now.with_timezone(&clock_zone).date_naive();
    [today.pred_opt()?, today, today.succ_opt()?]
        .into_iter()
        .flat_map(|day| match clock_zone.from_local_datetime(&day.and_time(time)) {
            LocalResult::Single(at) => vec![at],
            LocalResult::Ambiguous(first, second) => vec![first, second],
            // The time is in the hour the clocks skip: no such moment on that day.
            LocalResult::None => vec![],
        })
        .map(|at| at.with_timezone(&Utc))
        .min_by_key(|at| {
            let away = *at - now;
            (away.abs(), away > Duration::zero())
        })
}

/// Seconds from `now` until `clock_time` ("HH:MM" on the clock of `clock_zone`), negative if it has gone; see
/// `clock_time_near`. Resolution is a minute, so it agrees with the clock on the display.
pub fn seconds_until(now: DateTime<Tz>, clock_time: &str, clock_zone: Tz) -> Option<i64> {
    Some(seconds_to(now, clock_time_near(now, clock_time, clock_zone)?))
}

/// Seconds from `now` until `at`, counted from the start of this minute, negative if it has gone.
pub fn seconds_to(now: DateTime<Tz>, at: DateTime<Utc>) -> i64 {
    let this_minute = now.timestamp().div_euclid(60) * 60;
    at.timestamp() - this_minute
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
    use chrono_tz::Europe::London;

    use super::*;

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Tz> {
        London
            .with_ymd_and_hms(2024, 1, 10, hour, minute, second)
            .unwrap()
    }

    #[test]
    fn counts_whole_clock_minutes_ignoring_seconds() {
        assert_eq!(seconds_until(at(9, 3, 40), "09:09", London), Some(6 * 60));
    }

    #[test]
    fn wraps_past_midnight() {
        assert_eq!(seconds_until(at(23, 55, 0), "00:10", London), Some(15 * 60));
    }

    #[test]
    fn a_time_before_midnight_has_gone_after_it() {
        assert_eq!(seconds_until(at(0, 5, 0), "23:55", London), Some(-10 * 60));
        assert_eq!(seconds_until(at(0, 0, 0), "23:59", London), Some(-60));
    }

    #[test]
    fn the_nearest_day_is_chosen_at_the_edges() {
        assert_eq!(seconds_until(at(12, 0, 0), "12:00", London), Some(0));
        // Just under twelve hours ahead stays ahead; twelve hours or more behind is the next day.
        assert_eq!(
            seconds_until(at(12, 0, 0), "23:59", London),
            Some(11 * 3600 + 59 * 60)
        );
        assert_eq!(seconds_until(at(12, 0, 0), "00:00", London), Some(-12 * 3600));
    }

    #[test]
    fn a_time_just_gone_is_negative() {
        assert_eq!(seconds_until(at(9, 10, 0), "09:08", London), Some(-2 * 60));
    }

    #[test]
    fn rejects_text_that_is_not_a_time() {
        assert_eq!(seconds_until(at(9, 0, 0), "Delayed", London), None);
    }

    fn utc_at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<Tz> {
        Utc.with_ymd_and_hms(y, m, d, h, min, s).unwrap().with_timezone(&London)
    }

    #[test]
    fn a_countdown_is_real_time_across_the_hour_the_clocks_go_forward() {
        // 29 March 2026: 01:00 GMT becomes 02:00 BST. At 00:55 GMT, 02:05 BST is 10 minutes away, not 70.
        assert_eq!(seconds_until(utc_at(2026, 3, 29, 0, 55, 0), "02:05", London), Some(10 * 60));
        // And 00:55 was five minutes ago at 01:00 BST.
        assert_eq!(seconds_until(utc_at(2026, 3, 29, 1, 0, 0), "00:55", London), Some(-5 * 60));
    }

    #[test]
    fn a_countdown_is_real_time_across_the_hour_the_clocks_go_back() {
        // 25 October 2026: 02:00 BST becomes 01:00 GMT, so 01:00 to 02:00 is on the clock twice.
        // The first pass is 00:00 to 01:00 UTC, the second 01:00 to 02:00 UTC.
        // At 01:50 BST (00:50 UTC) a train at 01:10 is the one 20 minutes later, in the second pass, not 40 ago.
        assert_eq!(seconds_until(utc_at(2026, 10, 25, 0, 50, 0), "01:10", London), Some(20 * 60));
        // A train at 01:55 is 5 minutes away, still in the first pass.
        assert_eq!(seconds_until(utc_at(2026, 10, 25, 0, 50, 0), "01:55", London), Some(5 * 60));
        // At 01:05 BST (00:05 UTC), 01:10 is 5 minutes away, in the first pass.
        assert_eq!(seconds_until(utc_at(2026, 10, 25, 0, 5, 0), "01:10", London), Some(5 * 60));
        // At 01:05 GMT (01:05 UTC), 01:00 has just gone, in the second pass, not 65 minutes ago.
        assert_eq!(seconds_until(utc_at(2026, 10, 25, 1, 5, 0), "01:00", London), Some(-5 * 60));
    }

    #[test]
    fn a_time_read_on_the_providers_clock_counts_down_correctly_for_a_display_elsewhere() {
        use chrono_tz::Australia::Sydney;
        // 15 June 2026, 08:00 UTC: 09:00 in London (BST), 18:00 in Sydney.
        let display = Utc.with_ymd_and_hms(2026, 6, 15, 8, 0, 0).unwrap().with_timezone(&Sydney);
        // A UK board's 09:10 is ten minutes away. Read on the display's clock it would be 15 hours ago or ahead.
        assert_eq!(seconds_until(display, "09:10", London), Some(10 * 60));
        assert_eq!(seconds_until(display, "09:10", Sydney), Some(-8 * 3600 - 50 * 60));
        // The same moment, as the UK board's time shown on the display's clock.
        let at = clock_time_near(display, "09:10", London).unwrap();
        assert_eq!(at.with_timezone(&Sydney).format("%H:%M").to_string(), "18:10");
    }

    #[test]
    fn a_time_in_the_hour_that_does_not_exist_is_read_as_the_nearest_day_that_has_it() {
        // 01:30 doesn't exist on 29 March 2026 in London; yesterday's and tomorrow's do, and one is nearest.
        let now = utc_at(2026, 3, 29, 0, 30, 0);
        let at = clock_time_near(now, "01:30", London).unwrap();
        assert!(at != Utc.with_ymd_and_hms(2026, 3, 29, 1, 30, 0).unwrap());
        assert!((at - now.with_timezone(&Utc)).abs() <= Duration::hours(24));
        // Nothing parseable gives nothing.
        assert_eq!(clock_time_near(now, "late", London), None);
    }

    #[test]
    fn a_zone_with_a_half_hour_offset_and_none_to_change_just_works() {
        use chrono_tz::Asia::Colombo;
        let now = Utc.with_ymd_and_hms(2026, 6, 15, 3, 30, 0).unwrap().with_timezone(&Colombo); // 09:00 in Colombo
        assert_eq!(seconds_until(now, "09:15", Colombo), Some(15 * 60));
        assert_eq!(seconds_until(now, "08:45", Colombo), Some(-15 * 60));
        // 09:00 in Colombo is 04:30 in London, so a UK 09:15 is 4 h 45 min away.
        assert_eq!(seconds_until(now, "09:15", London), Some((4 * 60 + 45) * 60));
    }

    fn service(
        time: &str,
        status: DepartureStatus,
        expected: &str,
        countdown: &str,
    ) -> DepartureService {
        DepartureService::new(
            time.into(),
            "X".into(),
            status,
            expected.into(),
            countdown.into(),
        )
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
        let countdowns: Vec<_> = later
            .services
            .iter()
            .map(|s| s.countdown.as_str())
            .collect();
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
        let countdowns: Vec<_> = later
            .services
            .iter()
            .map(|s| s.countdown.as_str())
            .collect();
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

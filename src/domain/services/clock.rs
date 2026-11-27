use chrono::DateTime;
use chrono_tz::Tz;
use jiff::{SignedDuration, Timestamp};

/// What time it is. Asked for instead of read from the system, so what depends on the time
/// (a frame's clock and countdowns, whether an image is stale) can be tested at a chosen moment.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Tz>;

    /// The same moment as `now`, without a zone: for what is stored, compared and sent as a point in time.
    fn instant(&self) -> Timestamp {
        instant_of(self.now())
    }
}

/// The moment `at` is, without its zone. (Until the clock itself is jiff's, this is how the two meet.)
pub fn instant_of(at: DateTime<Tz>) -> Timestamp {
    Timestamp::from_second(at.timestamp()).expect("a clock reads a date jiff can hold")
}

/// `at` as the time on the wall in `zone`. (Until the clock itself is jiff's, this is how the two meet.)
pub fn in_zone(at: Timestamp, zone: Tz) -> DateTime<Tz> {
    use chrono::TimeZone;
    zone.timestamp_opt(at.as_second(), at.subsec_nanosecond() as u32)
        .single()
        .expect("every moment has a time in a zone")
}

/// A whole number of days as a length of time, counting every day as 24 hours (which is what a certificate's or
/// a session's lifetime means, whatever the clocks do).
pub const fn from_days(days: i64) -> SignedDuration {
    SignedDuration::from_hours(24 * days)
}

/// A date before which this program had not been written. A clock reading earlier than this has not been
/// set (a machine without a real-time clock starts at 1970 until a time service sets it), so no certificate
/// is made from it: one dated from such a clock would be refused by every display whose clock is right, or
/// would end the moment the clock was corrected.
pub fn earliest_plausible() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().expect("a valid date")
}

/// The words for a clock that reads before `earliest_plausible`.
pub fn implausible(now: Timestamp) -> String {
    format!(
        "the system clock reads {now}, which can't be right; not making certificates until it is set"
    )
}

/// A moment in UTC, for tests: `utc(2026, 10, 8, 12, 0, 0)`.
#[cfg(test)]
pub fn utc(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> Timestamp {
    jiff::civil::date(year, month, day)
        .at(hour, minute, second, 0)
        .to_zoned_datetime(jiff::tz::TimeZone::UTC)
        .expect("a valid moment")
        .timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the files on disk and the API held while this was chrono's: instants as RFC 3339 text, in UTC, with
    /// as many fractional digits as the moment needed.
    #[test]
    fn instants_written_when_this_was_chrono_still_read() {
        let read = |text: &str| serde_json::from_str::<Timestamp>(&format!("\"{text}\"")).unwrap();
        assert_eq!(read("2026-10-08T12:00:00Z"), utc(2026, 10, 8, 12, 0, 0));
        assert_eq!(
            read("2026-10-08T12:00:00.250Z"),
            utc(2026, 10, 8, 12, 0, 0) + SignedDuration::from_millis(250)
        );
        assert_eq!(
            read("2026-10-08T12:00:00.123456789Z").subsec_nanosecond(),
            123_456_789
        );
        assert_eq!(
            read("2026-10-08T13:00:00+01:00"),
            utc(2026, 10, 8, 12, 0, 0)
        );
    }

    #[test]
    fn instants_are_written_as_they_were() {
        let text = |at: Timestamp| serde_json::to_string(&at).unwrap();
        assert_eq!(text(utc(2026, 10, 8, 12, 0, 0)), "\"2026-10-08T12:00:00Z\"");
        assert_eq!(
            text(utc(2026, 10, 8, 12, 0, 0) + SignedDuration::from_millis(250)),
            "\"2026-10-08T12:00:00.25Z\""
        );
    }

    #[test]
    fn a_day_is_twenty_four_hours() {
        assert_eq!(from_days(2), SignedDuration::from_hours(48));
    }
}

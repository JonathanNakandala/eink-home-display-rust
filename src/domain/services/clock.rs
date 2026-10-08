use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// What time it is. Asked for instead of read from the system, so what depends on the time
/// (a frame's clock and countdowns, whether an image is stale) can be tested at a chosen moment.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Tz>;
}

/// A date before which this program had not been written. A clock reading earlier than this has not been
/// set (a machine without a real-time clock starts at 1970 until a time service sets it), so no certificate
/// is made from it: one dated from such a clock would be refused by every display whose clock is right, or
/// would end the moment the clock was corrected.
pub fn earliest_plausible() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("a valid date")
}

/// The words for a clock that reads before `earliest_plausible`.
pub fn implausible(now: DateTime<Utc>) -> String {
    format!(
        "the system clock reads {now}, which can't be right; not making certificates until it is set"
    )
}

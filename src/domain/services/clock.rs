use chrono::DateTime;
use chrono_tz::Tz;

/// What time it is. Asked for instead of read from the system, so what depends on the time
/// (a frame's clock and countdowns, whether an image is stale) can be tested at a chosen moment.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Tz>;
}

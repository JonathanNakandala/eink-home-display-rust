use crate::domain::services::clock::Clock;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};

/// The system clock, reading the time in `zone`. The zone is chosen by whoever builds it (the configuration, or the host's
/// own when none is set), not taken from the process's environment at every call.
pub struct SystemClock {
    zone: TimeZone,
}

impl SystemClock {
    pub fn new(zone: TimeZone) -> Self {
        Self { zone }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> ZonedDateTime {
        Timestamp::now().to_zoned_datetime(self.zone.clone())
    }
}

/// A clock that only moves when told to, for tests.
#[cfg(test)]
pub struct FixedClock(std::sync::Mutex<ZonedDateTime>);

#[cfg(test)]
impl FixedClock {
    pub fn at(now: ZonedDateTime) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(std::sync::Mutex::new(now)))
    }

    pub fn set(&self, now: ZonedDateTime) {
        *self.0.lock().unwrap() = now;
    }
}

#[cfg(test)]
impl Clock for FixedClock {
    fn now(&self) -> ZonedDateTime {
        self.0.lock().unwrap().clone()
    }
}

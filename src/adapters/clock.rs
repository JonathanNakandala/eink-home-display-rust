use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::domain::services::clock::Clock;

/// The system clock, reading the time in `zone`. The zone is chosen by whoever builds it (the configuration, or the host's
/// own when none is set), not taken from the process's environment at every call.
pub struct SystemClock {
    zone: Tz,
}

impl SystemClock {
    pub fn new(zone: Tz) -> Self {
        Self { zone }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.zone)
    }
}

/// A clock that only moves when told to, for tests.
#[cfg(test)]
pub struct FixedClock(std::sync::Mutex<DateTime<Tz>>);

#[cfg(test)]
impl FixedClock {
    pub fn at(now: DateTime<Tz>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(std::sync::Mutex::new(now)))
    }

    pub fn set(&self, now: DateTime<Tz>) {
        *self.0.lock().unwrap() = now;
    }
}

#[cfg(test)]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Tz> {
        *self.0.lock().unwrap()
    }
}

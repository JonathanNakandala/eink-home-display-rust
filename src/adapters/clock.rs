use chrono::{DateTime, Local};

use crate::domain::services::clock::Clock;

/// The system clock, in local time.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }
}

/// A clock that only moves when told to, for tests.
#[cfg(test)]
pub struct FixedClock(std::sync::Mutex<DateTime<Local>>);

#[cfg(test)]
impl FixedClock {
    pub fn at(now: DateTime<Local>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(std::sync::Mutex::new(now)))
    }

    pub fn set(&self, now: DateTime<Local>) {
        *self.0.lock().unwrap() = now;
    }
}

#[cfg(test)]
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Local> {
        *self.0.lock().unwrap()
    }
}

//! Keeping the last good result of a data source, so a source that has just failed can be
//! shown from what it said a little while ago, labelled with how old that is, instead of
//! being left off the display.

use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Duration, Local};

use crate::domain::models::source_error::SourceError;

#[derive(Debug, Clone, PartialEq)]
pub enum Fetched<T> {
    /// Just fetched.
    Fresh(T),
    /// The fetch failed; this is from `age` ago, within the limit for it.
    Stale { value: T, age: Duration },
    /// The fetch failed and there is nothing recent enough to show. `reason` is a few words for the display.
    Unavailable { reason: &'static str },
}

pub struct LastGood<T> {
    slot: Mutex<Option<(T, DateTime<Local>)>>,
}

impl<T> Default for LastGood<T> {
    fn default() -> Self {
        Self { slot: Mutex::new(None) }
    }
}

impl<T: Clone> LastGood<T> {
    /// Remembers a success, or falls back on the remembered one if it is no older than `max_age`.
    /// `source` names the data in the log.
    pub fn resolve(
        &self,
        result: Result<T, SourceError>,
        now: DateTime<Local>,
        max_age: Duration,
        source: &str,
    ) -> Fetched<T> {
        // The slot is plain data and each update leaves it whole, so a panic elsewhere while the lock was
        // held is no reason to stop remembering.
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let error = match result {
            Ok(value) => {
                *slot = Some((value.clone(), now));
                return Fetched::Fresh(value);
            }
            Err(error) => error,
        };
        let reason = error.reason();
        match slot.as_ref() {
            // From later than now: the clock has been set back since, so there is no telling how old this
            // is. Better off the display than shown with an age that is made up, and dropped so that the
            // clock catching up can't bring it back looking fresh.
            Some((_, fetched_at)) if *fetched_at > now => {
                report(source, &error, "its last data is from the future (the clock was set back), so it is dropped");
                *slot = None;
                Fetched::Unavailable { reason }
            }
            Some((value, fetched_at)) if now - *fetched_at <= max_age => {
                let age = (now - *fetched_at).max(Duration::zero());
                report(source, &error, &format!("showing data from {} ago", format_age(age)));
                Fetched::Stale { value: value.clone(), age }
            }
            Some((_, fetched_at)) => {
                let detail = format!("its last data is {} old, over the limit", format_age(now - *fetched_at));
                report(source, &error, &detail);
                Fetched::Unavailable { reason }
            }
            None => {
                report(source, &error, "and there is no earlier data");
                Fetched::Unavailable { reason }
            }
        }
    }
}

/// A failure that will probably clear is a warning; one that won't, because the key or the request
/// is wrong, is an error, since waiting won't help.
fn report(source: &str, error: &SourceError, consequence: &str) {
    if error.needs_attention() {
        log::error!("{source} failed and needs attention: {error:#}; {consequence}");
    } else {
        log::warn!("{source} failed: {error:#}; {consequence}");
    }
}

/// "under 1 min", "8 min", "1 h 20 min".
pub fn format_age(age: Duration) -> String {
    let minutes = age.num_minutes();
    match minutes {
        ..=0 => "under 1 min".to_owned(),
        1..=59 => format!("{minutes} min"),
        _ => format!("{} h {} min", minutes / 60, minutes % 60),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, 0).unwrap()
    }

    const LIMIT: Duration = Duration::minutes(15);

    fn down() -> SourceError {
        SourceError::Upstream { status: 503 }
    }

    #[test]
    fn a_success_is_fresh_and_remembered() {
        let last = LastGood::default();
        assert_eq!(last.resolve(Ok(1), at(12, 0), LIMIT, "x"), Fetched::Fresh(1));
        assert_eq!(
            last.resolve(Err(down()), at(12, 5), LIMIT, "x"),
            Fetched::Stale { value: 1, age: Duration::minutes(5) }
        );
    }

    #[test]
    fn data_older_than_the_limit_is_unavailable() {
        let last = LastGood::default();
        last.resolve(Ok(1), at(12, 0), LIMIT, "x");
        assert_eq!(last.resolve(Err(down()), at(12, 15), LIMIT, "x"), Fetched::Stale { value: 1, age: LIMIT });
        assert_eq!(
            last.resolve(Err(down()), at(12, 16), LIMIT, "x"),
            Fetched::Unavailable { reason: "service error" }
        );
    }

    #[test]
    fn a_failure_with_nothing_remembered_is_unavailable() {
        let last: LastGood<i32> = LastGood::default();
        assert_eq!(last.resolve(Err(down()), at(12, 0), LIMIT, "x"), Fetched::Unavailable { reason: "service error" });
    }

    #[test]
    fn the_reason_comes_from_the_kind_of_failure() {
        let last: LastGood<i32> = LastGood::default();
        let unavailable = |error| last.resolve(Err(error), at(12, 0), LIMIT, "x");
        assert_eq!(unavailable(SourceError::Unauthorized { status: 401 }), Fetched::Unavailable { reason: "key rejected" });
        assert_eq!(unavailable(SourceError::Timeout), Fetched::Unavailable { reason: "timed out" });
    }

    #[test]
    fn a_new_success_replaces_the_old_and_resets_the_age() {
        let last = LastGood::default();
        last.resolve(Ok(1), at(12, 0), LIMIT, "x");
        last.resolve(Ok(2), at(12, 10), LIMIT, "x");
        assert_eq!(
            last.resolve(Err(down()), at(12, 20), LIMIT, "x"),
            Fetched::Stale { value: 2, age: Duration::minutes(10) }
        );
    }

    #[test]
    fn ages_are_worded_for_a_glance() {
        assert_eq!(format_age(Duration::seconds(30)), "under 1 min");
        assert_eq!(format_age(Duration::minutes(8)), "8 min");
        assert_eq!(format_age(Duration::minutes(80)), "1 h 20 min");
    }

    #[test]
    fn data_from_the_future_after_a_clock_set_back_is_dropped_not_shown_as_fresh() {
        let slot = LastGood::default();
        slot.resolve(Ok(1), at(15, 0), LIMIT, "t");

        // Three hours back, and the source is down: the data's age can't be known.
        let fetched = slot.resolve(Err(SourceError::Timeout), at(12, 0), LIMIT, "t");
        assert_eq!(fetched, Fetched::Unavailable { reason: "timed out" });

        // And it stays gone when the clock catches up to 15:05, within the limit of when it was fetched.
        let later = slot.resolve(Err(SourceError::Timeout), at(15, 5), LIMIT, "t");
        assert_eq!(later, Fetched::Unavailable { reason: "timed out" });

        // A source that answers again starts over.
        assert_eq!(slot.resolve(Ok(2), at(12, 10), LIMIT, "t"), Fetched::Fresh(2));
        assert_eq!(
            slot.resolve(Err(SourceError::Timeout), at(12, 15), LIMIT, "t"),
            Fetched::Stale { value: 2, age: Duration::minutes(5) }
        );
    }
}

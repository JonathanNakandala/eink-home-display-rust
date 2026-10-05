//! Keeping the last good result of a data source, so a source that has just failed can be
//! shown from what it said a little while ago, labelled with how old that is, instead of
//! being left off the display.

use std::sync::Mutex;

use chrono::{DateTime, Duration, Local};

#[derive(Debug, Clone, PartialEq)]
pub enum Fetched<T> {
    /// Just fetched.
    Fresh(T),
    /// The fetch failed; this is from `age` ago, within the limit for it.
    Stale { value: T, age: Duration },
    /// The fetch failed and there is nothing recent enough to show.
    Unavailable,
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
    pub fn resolve(&self, result: anyhow::Result<T>, now: DateTime<Local>, max_age: Duration, source: &str) -> Fetched<T> {
        let mut slot = self.slot.lock().unwrap();
        match result {
            Ok(value) => {
                *slot = Some((value.clone(), now));
                Fetched::Fresh(value)
            }
            Err(error) => match slot.as_ref() {
                Some((value, fetched_at)) if now - *fetched_at <= max_age => {
                    let age = (now - *fetched_at).max(Duration::zero());
                    log::warn!("{source} failed, showing data from {} ago: {error:#}", format_age(age));
                    Fetched::Stale { value: value.clone(), age }
                }
                Some((_, fetched_at)) => {
                    log::warn!(
                        "{source} failed and its last data is {} old, over the limit: {error:#}",
                        format_age(now - *fetched_at)
                    );
                    Fetched::Unavailable
                }
                None => {
                    log::warn!("{source} failed and there is no earlier data: {error:#}");
                    Fetched::Unavailable
                }
            },
        }
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
    use anyhow::anyhow;
    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, 0).unwrap()
    }

    const LIMIT: Duration = Duration::minutes(15);

    #[test]
    fn a_success_is_fresh_and_remembered() {
        let last = LastGood::default();
        assert_eq!(last.resolve(Ok(1), at(12, 0), LIMIT, "x"), Fetched::Fresh(1));
        assert_eq!(
            last.resolve(Err(anyhow!("down")), at(12, 5), LIMIT, "x"),
            Fetched::Stale { value: 1, age: Duration::minutes(5) }
        );
    }

    #[test]
    fn data_older_than_the_limit_is_unavailable() {
        let last = LastGood::default();
        last.resolve(Ok(1), at(12, 0), LIMIT, "x");
        assert_eq!(last.resolve(Err(anyhow!("down")), at(12, 15), LIMIT, "x"), Fetched::Stale { value: 1, age: LIMIT });
        assert_eq!(last.resolve(Err(anyhow!("down")), at(12, 16), LIMIT, "x"), Fetched::Unavailable);
    }

    #[test]
    fn a_failure_with_nothing_remembered_is_unavailable() {
        let last: LastGood<i32> = LastGood::default();
        assert_eq!(last.resolve(Err(anyhow!("down")), at(12, 0), LIMIT, "x"), Fetched::Unavailable);
    }

    #[test]
    fn a_new_success_replaces_the_old_and_resets_the_age() {
        let last = LastGood::default();
        last.resolve(Ok(1), at(12, 0), LIMIT, "x");
        last.resolve(Ok(2), at(12, 10), LIMIT, "x");
        assert_eq!(
            last.resolve(Err(anyhow!("down")), at(12, 20), LIMIT, "x"),
            Fetched::Stale { value: 2, age: Duration::minutes(10) }
        );
    }

    #[test]
    fn ages_are_worded_for_a_glance() {
        assert_eq!(format_age(Duration::seconds(30)), "under 1 min");
        assert_eq!(format_age(Duration::minutes(8)), "8 min");
        assert_eq!(format_age(Duration::minutes(80)), "1 h 20 min");
    }
}

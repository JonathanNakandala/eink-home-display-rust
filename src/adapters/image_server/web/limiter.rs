//! How many times a source may fail to show a good code or token before it is turned away for a while.
//!
//! A token is 256 random bits, so guessing one is hopeless; this is not what protects it. It is for the noise: a script
//! trying codes, or a browser left with a revoked token asking every few seconds, would otherwise fill the log and cost a
//! lookup each time. Nothing is kept on disk: a restart forgets, which is fine.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::adapters::source::{Source, source_of};

/// The failures from one source in a window before it is turned away.
const MOST_FAILURES: u32 = 10;
/// How long failures are counted for, and how long a source that has used them up waits.
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// The most sources remembered, so that the table can't be made to grow by spreading across addresses. When it is full of
/// live entries, a source not already in it is not counted (it is let through), which is the safe way round for a limit
/// that is only a nuisance filter.
const MAX_SOURCES: usize = 1024;

pub struct FailureLimiter {
    most: u32,
    window: Duration,
    table: Mutex<HashMap<Source, Entry>>,
}

struct Entry {
    failures: u32,
    since: Instant,
}

impl Default for FailureLimiter {
    fn default() -> Self {
        Self::new(MOST_FAILURES, WINDOW)
    }
}

impl FailureLimiter {
    pub fn new(most: u32, window: Duration) -> Self {
        Self {
            most,
            window,
            table: Mutex::new(HashMap::new()),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<Source, Entry>> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How long `source` has to wait, if it has used up its failures.
    pub fn wait(&self, source: std::net::IpAddr) -> Option<Duration> {
        let source = source_of(source);
        let table = self.table();
        let entry = table.get(&source)?;
        let elapsed = entry.since.elapsed();
        (entry.failures >= self.most && elapsed < self.window).then(|| self.window - elapsed)
    }

    /// Counts a failure from `source`.
    pub fn failed(&self, source: std::net::IpAddr) {
        let source = source_of(source);
        let mut table = self.table();
        let now = Instant::now();
        table.retain(|_, entry| now.duration_since(entry.since) < self.window);
        if !table.contains_key(&source) && table.len() >= MAX_SOURCES {
            return;
        }
        let entry = table.entry(source).or_insert(Entry {
            failures: 0,
            since: now,
        });
        entry.failures += 1;
    }

    /// A good code or token from `source`: what it got wrong before is forgiven.
    pub fn succeeded(&self, source: std::net::IpAddr) {
        self.table().remove(&source_of(source));
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn a_source_is_turned_away_after_its_failures_and_let_back_after_the_window() {
        let limiter = FailureLimiter::new(3, Duration::from_secs(60));
        let source = address("192.168.1.5");

        for _ in 0..2 {
            limiter.failed(source);
        }
        assert!(limiter.wait(source).is_none(), "two of three is not enough");
        limiter.failed(source);
        let wait = limiter.wait(source).expect("three is");
        assert!(wait <= Duration::from_secs(60) && wait > Duration::from_secs(50));

        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(limiter.wait(source).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn one_source_failing_does_not_turn_another_away_and_a_good_one_is_forgiven() {
        let limiter = FailureLimiter::new(2, Duration::from_secs(60));
        let (noisy, quiet) = (address("192.168.1.5"), address("192.168.1.6"));

        limiter.failed(noisy);
        limiter.failed(noisy);
        limiter.failed(quiet);

        assert!(limiter.wait(noisy).is_some());
        assert!(limiter.wait(quiet).is_none());
        limiter.succeeded(noisy);
        assert!(limiter.wait(noisy).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn an_ipv6_network_is_one_source_and_a_mapped_ipv4_is_the_ipv4_one() {
        let limiter = FailureLimiter::new(2, Duration::from_secs(60));

        limiter.failed(address("2001:db8::1"));
        limiter.failed(address("2001:db8::2"));
        assert!(
            limiter.wait(address("2001:db8::ffff")).is_some(),
            "the same /64"
        );
        assert!(limiter.wait(address("2001:db9::1")).is_none());

        limiter.failed(address("::ffff:10.0.0.1"));
        limiter.failed(address("10.0.0.1"));
        assert!(limiter.wait(address("10.0.0.1")).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn the_table_has_a_bound_and_old_entries_make_room() {
        let limiter = FailureLimiter::new(1, Duration::from_secs(60));
        for i in 0..MAX_SOURCES {
            limiter.failed(address(&format!("10.{}.{}.1", i / 250, i % 250)));
        }
        let late = address("172.16.0.1");

        limiter.failed(late);
        assert!(
            limiter.wait(late).is_none(),
            "full of live entries: not counted, let through"
        );

        tokio::time::advance(Duration::from_secs(61)).await;
        limiter.failed(late);
        assert!(limiter.wait(late).is_some(), "room once the old ones lapse");
    }
}

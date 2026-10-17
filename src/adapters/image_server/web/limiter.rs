//! How many times a source may fail to show a good code or token before it is turned away for a while, and how many streams a
//! source may hold open.
//!
//! A token is 256 random bits, so guessing one is hopeless; the first is not what protects it. It is for the noise: a script
//! trying codes, or a browser left with a revoked token asking every few seconds, would otherwise fill the log and cost a
//! lookup each time. A source for it is an address, never a whole IPv6 /64: a home network gives every device an address in
//! the same /64, and one stale tablet must not turn the others away. (A stranger rotating addresses to get round it gets nothing
//! for it, since there was nothing to guess.) Nothing is kept on disk: a restart forgets, which is fine.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::adapters::source::{Source, source_of};

/// The address, as it is: an IPv4 address that arrived as an IPv6 one is the IPv4 one, and that is all.
fn exact(address: std::net::IpAddr) -> std::net::IpAddr {
    match address {
        std::net::IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map_or(std::net::IpAddr::V6(v6), std::net::IpAddr::V4),
        v4 => v4,
    }
}

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
        let source = exact(source);
        let table = self.table();
        let entry = table.get(&source)?;
        let elapsed = entry.since.elapsed();
        (entry.failures >= self.most && elapsed < self.window).then(|| self.window - elapsed)
    }

    /// Counts a failure from `source`.
    pub fn failed(&self, source: std::net::IpAddr) {
        let source = exact(source);
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
        self.table().remove(&exact(source));
    }
}

/// How many streams of changes (`/view/api/events`) may be open, in all and from one source.
///
/// A stream is a connection held for a minute and a half, and the HTTPS side has only so many connections, in all and for one
/// source (and a source there is a whole IPv6 /64). Pages that each held one for as long as they were open could use them
/// all, and then every request of every other page, and of a display, would be turned away at the door. So a stream is
/// let in only while there are streams to spare, and one that is not still gets everything else: the page carries on asking
/// on its own, and asks for a stream again after a wait.
pub struct StreamBudget {
    most: usize,
    most_per_source: usize,
    open: Mutex<Open>,
}

#[derive(Default)]
struct Open {
    all: usize,
    by_source: HashMap<Source, usize>,
}

/// A place for a stream, given back when it is dropped.
pub struct StreamPermit {
    budget: std::sync::Arc<StreamBudget>,
    source: Option<Source>,
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        let mut open = self
            .budget
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        open.all = open.all.saturating_sub(1);
        if let Some(source) = self.source
            && let Some(held) = open.by_source.get_mut(&source)
        {
            *held -= 1;
            if *held == 0 {
                open.by_source.remove(&source);
            }
        }
    }
}

impl StreamBudget {
    pub fn new(most: usize, most_per_source: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            most,
            most_per_source,
            open: Mutex::new(Open::default()),
        })
    }

    /// A place for a stream for `source` (none if it is not known, as over plain HTTP, when only the total counts), or `None`
    /// if there is none to spare. The source is counted as the HTTPS side counts its connections: a whole IPv6 /64 is one,
    /// since it is that share of connections that a stream is taking.
    pub fn acquire(
        self: &std::sync::Arc<Self>,
        source: Option<std::net::IpAddr>,
    ) -> Option<StreamPermit> {
        let source = source.map(source_of);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.all >= self.most {
            return None;
        }
        if let Some(source) = source {
            let held = open.by_source.entry(source).or_insert(0);
            if *held >= self.most_per_source {
                return None;
            }
            *held += 1;
        }
        open.all += 1;
        drop(open);
        Some(StreamPermit {
            budget: std::sync::Arc::clone(self),
            source,
        })
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
    async fn each_address_is_its_own_source_so_one_device_failing_does_not_turn_away_the_rest_of_a_home_network()
     {
        let limiter = FailureLimiter::new(2, Duration::from_secs(60));

        // A flat home network gives every device an address in the same IPv6 /64: one stale tablet must not lock out the rest.
        limiter.failed(address("2001:db8::1"));
        limiter.failed(address("2001:db8::1"));
        assert!(limiter.wait(address("2001:db8::1")).is_some());
        assert!(
            limiter.wait(address("2001:db8::2")).is_none(),
            "its neighbour in the /64"
        );
        assert!(limiter.wait(address("2001:db8::ffff")).is_none());

        // An IPv4 address that reaches a dual-stack server as an IPv6 one is still the IPv4 one.
        limiter.failed(address("::ffff:10.0.0.1"));
        limiter.failed(address("10.0.0.1"));
        assert!(limiter.wait(address("10.0.0.1")).is_some());
        assert!(limiter.wait(address("::ffff:10.0.0.1")).is_some());
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

    #[test]
    fn a_source_holds_only_its_share_of_streams_and_the_total_is_bounded() {
        let budget = StreamBudget::new(3, 2);
        let (a, b) = (address("192.168.1.5"), address("192.168.1.6"));

        let first = budget.acquire(Some(a)).expect("one");
        let second = budget.acquire(Some(a)).expect("two");
        assert!(budget.acquire(Some(a)).is_none(), "a third from one source");
        // Another source has its own share, until the total is used.
        let other = budget.acquire(Some(b)).expect("another source");
        assert!(budget.acquire(Some(b)).is_none(), "the total is three");

        // A place given back is one to spare again, for whoever asks.
        drop(first);
        assert!(budget.acquire(Some(b)).is_some());
        drop((second, other));
    }

    #[test]
    fn a_whole_ipv6_network_is_one_source_here_as_it_is_for_the_connections_streams_use() {
        let budget = StreamBudget::new(8, 2);

        let _one = budget.acquire(Some(address("2001:db8::1"))).unwrap();
        let _two = budget.acquire(Some(address("2001:db8::2"))).unwrap();

        assert!(budget.acquire(Some(address("2001:db8::3"))).is_none());
        assert!(
            budget.acquire(Some(address("2001:db9::1"))).is_some(),
            "another network"
        );
        // And an IPv4 address that came in as IPv6 is the IPv4 one.
        let _v4 = budget.acquire(Some(address("10.0.0.1"))).unwrap();
        let _mapped = budget.acquire(Some(address("::ffff:10.0.0.1"))).unwrap();
        assert!(budget.acquire(Some(address("10.0.0.1"))).is_none());
    }

    #[test]
    fn with_no_source_known_only_the_total_counts_and_nothing_leaks_when_permits_are_dropped() {
        let budget = StreamBudget::new(2, 1);

        let a = budget.acquire(None).unwrap();
        let b = budget.acquire(None).unwrap();
        assert!(budget.acquire(None).is_none());
        drop((a, b));

        // All the way back: the whole budget is there again, for one source or none.
        let again = [
            budget.acquire(Some(address("10.0.0.1"))),
            budget.acquire(Some(address("10.0.0.2"))),
        ];
        assert!(again.iter().all(Option::is_some));
        assert!(budget.acquire(None).is_none());
    }
}

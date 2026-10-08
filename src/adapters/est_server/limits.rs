//! What one source on the network may take of the server, because the endpoint answers anyone.
//!
//! Joining is open to whoever can reach the port while the owner's window is open, and the places for waiting
//! requests are few (`MAX_PENDING`). Without a limit per source, one stranger could take all of them, or every
//! connection, and the display being paired would be turned away. Three things are kept in step here:
//!
//! - the connections a source holds at once;
//! - the different display names a source has asked to join as. A display asks again and again under one name, so
//!   that costs nothing; a source asking under many names is filling the places, not joining;
//! - how much of the log a stream of refusals may take.
//!
//! A source is an address, except that all of an IPv6 /64 is one (a host is given the whole of one, so counting
//! addresses would give an attacker as many as it likes), and an IPv4 address written as IPv6 is the IPv4 one.
//! Nothing is kept on disk: a restart forgets, which is fine because the owner is there when one happens.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::domain::models::device_id::DeviceId;

/// How long a name counts against its source: as long as a request waits for the owner (see `REQUEST_LIFETIME`).
const NAME_MEMORY: Duration = Duration::from_secs(24 * 60 * 60);
/// What a source that asked under too many names is told to wait.
const NAME_RETRY: Duration = Duration::from_secs(10 * 60);
/// The most sources remembered at once, so that the memory used can't be made to grow by spreading across addresses.
/// When it is full of live entries, a source not already in it is refused until some lapse.
const MAX_SOURCES: usize = 1024;
/// The refusals logged as warnings in a minute. The rest are counted and said once.
const WARNINGS_PER_MINUTE: u32 = 10;

pub type Source = IpAddr;

/// The source an address belongs to.
pub fn source_of(address: IpAddr) -> Source {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        },
        v4 => v4,
    }
}

pub struct Limits {
    max_per_source: usize,
    max_names: usize,
    connections: Mutex<HashMap<Source, usize>>,
    names: Mutex<HashMap<Source, Vec<(DeviceId, Instant)>>>,
    log: Mutex<Window>,
}

struct Window {
    started: Instant,
    warned: u32,
    held_back: u32,
}

/// A place taken by a connection, given back when it is dropped.
pub struct Held {
    limits: Arc<Limits>,
    source: Source,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut connections = lock(&self.limits.connections);
        if let Some(count) = connections.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                connections.remove(&self.source);
            }
        }
    }
}

impl Limits {
    pub fn new(max_connections_per_source: usize, max_names_per_source: usize) -> Arc<Self> {
        Arc::new(Self {
            max_per_source: max_connections_per_source,
            max_names: max_names_per_source,
            connections: Mutex::new(HashMap::new()),
            names: Mutex::new(HashMap::new()),
            log: Mutex::new(Window {
                started: Instant::now(),
                warned: 0,
                held_back: 0,
            }),
        })
    }

    /// A place for a connection from `address`, or `None` if its source already holds all it may.
    pub fn admit(self: &Arc<Self>, address: IpAddr) -> Option<Held> {
        let source = source_of(address);
        let mut connections = lock(&self.connections);
        // Bounded like the names: a source not already here is turned away when the table is full.
        if !connections.contains_key(&source) && connections.len() >= MAX_SOURCES {
            return None;
        }
        let count = connections.entry(source).or_insert(0);
        if *count >= self.max_per_source {
            return None;
        }
        *count += 1;
        Some(Held {
            limits: self.clone(),
            source,
        })
    }

    /// Notes that `address` asked to join as `device`. `Err` says how long to wait if that is one name too many for
    /// its source; a name it has already asked as is always let through, and does not count again.
    pub fn allow_name(&self, address: IpAddr, device: &DeviceId) -> Result<(), Duration> {
        let source = source_of(address);
        let now = Instant::now();
        let mut names = lock(&self.names);
        if names.len() >= MAX_SOURCES && !names.contains_key(&source) {
            names.retain(|_, asked| {
                asked.retain(|(_, at)| now.duration_since(*at) < NAME_MEMORY);
                !asked.is_empty()
            });
            if names.len() >= MAX_SOURCES {
                return Err(NAME_RETRY);
            }
        }
        let asked = names.entry(source).or_default();
        asked.retain(|(_, at)| now.duration_since(*at) < NAME_MEMORY);
        if asked.iter().any(|(name, _)| name == device) {
            return Ok(());
        }
        if asked.len() >= self.max_names {
            return Err(NAME_RETRY);
        }
        asked.push((device.clone(), now));
        Ok(())
    }

    /// Whether a refusal may be logged as a warning. A stream of them from strangers would otherwise fill the
    /// journal, so after a few in a minute the rest are counted, and the count is logged when the minute is over.
    pub fn may_warn(&self) -> bool {
        let now = Instant::now();
        let mut window = lock(&self.log);
        if now.duration_since(window.started) >= Duration::from_secs(60) {
            if window.held_back > 0 {
                log::warn!(
                    "{} more refusals in the last minute were not logged one by one",
                    window.held_back
                );
            }
            *window = Window {
                started: now,
                warned: 0,
                held_back: 0,
            };
        }
        if window.warned < WARNINGS_PER_MINUTE {
            window.warned += 1;
            true
        } else {
            window.held_back += 1;
            false
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn name(text: &str) -> DeviceId {
        DeviceId::parse(text).unwrap()
    }

    #[test]
    fn ipv6_addresses_in_one_64_are_one_source_and_mapped_ipv4_is_ipv4() {
        assert_eq!(
            source_of(ip("2001:db8:1:2:aaaa:bbbb:cccc:dddd")),
            source_of(ip("2001:db8:1:2::1"))
        );
        assert_ne!(
            source_of(ip("2001:db8:1:2::1")),
            source_of(ip("2001:db8:1:3::1"))
        );
        assert_eq!(source_of(ip("::ffff:192.0.2.7")), ip("192.0.2.7"));
        assert_eq!(source_of(ip("192.0.2.7")), ip("192.0.2.7"));
    }

    #[test]
    fn a_source_holds_only_so_many_connections_and_gets_them_back() {
        let limits = Limits::new(2, 4);
        let a = limits.admit(ip("192.0.2.1")).unwrap();
        let b = limits.admit(ip("192.0.2.1")).unwrap();
        assert!(limits.admit(ip("192.0.2.1")).is_none());
        // Another source is not affected, and neither is the same host over IPv6-mapped IPv4.
        let other = limits.admit(ip("192.0.2.2")).unwrap();
        assert!(limits.admit(ip("::ffff:192.0.2.1")).is_none());
        drop(a);
        let again = limits.admit(ip("192.0.2.1")).unwrap();
        assert!(limits.admit(ip("192.0.2.1")).is_none());
        drop((b, again, other));
        // Everything given back leaves nothing remembered.
        assert!(limits.connections.lock().unwrap().is_empty());
    }

    #[test]
    fn a_source_may_ask_under_a_few_names_and_asking_again_costs_nothing() {
        let limits = Limits::new(4, 2);
        let source = ip("192.0.2.1");
        for _ in 0..10 {
            assert!(limits.allow_name(source, &name("kitchen")).is_ok());
        }
        assert!(limits.allow_name(source, &name("hall")).is_ok());
        assert!(limits.allow_name(source, &name("study")).is_err());
        // The names it has are still let through, and another source has its own allowance.
        assert!(limits.allow_name(source, &name("kitchen")).is_ok());
        assert!(limits.allow_name(ip("192.0.2.2"), &name("study")).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_name_counts_against_its_source_for_a_day_and_no_longer() {
        let limits = Limits::new(4, 1);
        let source = ip("192.0.2.1");
        limits.allow_name(source, &name("kitchen")).unwrap();
        assert!(limits.allow_name(source, &name("hall")).is_err());
        tokio::time::advance(NAME_MEMORY - Duration::from_secs(1)).await;
        assert!(limits.allow_name(source, &name("hall")).is_err());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(limits.allow_name(source, &name("hall")).is_ok());
    }

    #[test]
    fn the_sources_remembered_are_bounded() {
        let limits = Limits::new(4, 4);
        for n in 0..MAX_SOURCES {
            let source = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + n as u32));
            limits.allow_name(source, &name("kitchen")).unwrap();
            let held = limits.admit(source);
            assert!(held.is_some());
            std::mem::forget(held);
        }
        let newcomer = ip("192.0.2.1");
        assert!(limits.allow_name(newcomer, &name("kitchen")).is_err());
        assert!(limits.admit(newcomer).is_none());
        // One already known is still served.
        assert!(limits.allow_name(ip("10.0.0.0"), &name("kitchen")).is_ok());
        assert_eq!(limits.names.lock().unwrap().len(), MAX_SOURCES);
    }

    #[tokio::test(start_paused = true)]
    async fn only_a_few_refusals_a_minute_are_logged_and_the_window_then_starts_again() {
        let limits = Limits::new(4, 4);
        for _ in 0..WARNINGS_PER_MINUTE {
            assert!(limits.may_warn());
        }
        assert!(!limits.may_warn());
        assert!(!limits.may_warn());
        assert_eq!(limits.log.lock().unwrap().held_back, 2);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(limits.may_warn());
        assert_eq!(limits.log.lock().unwrap().held_back, 0);
    }
}

//! How many TLS connections began with a full handshake and how many resumed an earlier session, so the
//! owner can see whether displays are resuming (which saves them the certificate exchange and its signatures),
//! and, for each display, how its own connections began and how long its last handshake took.
//!
//! The per-display figures are the server's own view, taken from the connection and not reported by the display, so
//! a display cannot misstate them: the kind of handshake, and the time from accepting the connection to the end of the
//! handshake. That time includes the display's own work (a slow chip shows) and the network between.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::domain::models::device_id::DeviceId;

/// More displays than a household has. What is kept per display is a few numbers, but the names come from certificates
/// the authority signed, so the table is bounded all the same.
const MAX_DEVICES: usize = 64;

#[derive(Debug, Default)]
pub struct Handshakes {
    full: AtomicU64,
    resumed: AtomicU64,
    devices: Mutex<HashMap<DeviceId, DeviceHandshakes>>,
}

/// How one display's connections have begun, since the server started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceHandshakes {
    pub full: u64,
    pub resumed: u64,
    /// Whether the latest one resumed a session.
    pub last_resumed: bool,
    /// How long the latest one took, from accepting the connection to the end of the handshake.
    pub last_handshake: Duration,
    pub last_at: DateTime<Utc>,
}

impl Handshakes {
    pub fn record(&self, resumed: bool) {
        let counter = if resumed { &self.resumed } else { &self.full };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// `(full, resumed)` since the server started.
    pub fn counts(&self) -> (u64, u64) {
        (
            self.full.load(Ordering::Relaxed),
            self.resumed.load(Ordering::Relaxed),
        )
    }

    /// A connection of `device` (known now, from its certificate) began this way and took this long.
    pub fn record_device(
        &self,
        device: &DeviceId,
        resumed: bool,
        handshake: Duration,
        at: DateTime<Utc>,
    ) {
        let mut devices = self.devices.lock().unwrap_or_else(PoisonError::into_inner);
        if devices.len() >= MAX_DEVICES && !devices.contains_key(device) {
            return;
        }
        let entry = devices.entry(device.clone()).or_insert(DeviceHandshakes {
            full: 0,
            resumed: 0,
            last_resumed: resumed,
            last_handshake: handshake,
            last_at: at,
        });
        if resumed {
            entry.resumed += 1;
        } else {
            entry.full += 1;
            if entry.last_resumed && entry.full + entry.resumed > 1 {
                log::debug!(
                    "{device} began a connection with a full handshake after resuming the one before"
                );
            }
        }
        entry.last_resumed = resumed;
        entry.last_handshake = handshake;
        entry.last_at = at;
    }

    /// How `device`'s connections have begun, if it has had one since the server started.
    pub fn device(&self, device: &DeviceId) -> Option<DeviceHandshakes> {
        self.devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(device)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 10, 12, minute, 0).unwrap()
    }

    #[test]
    fn each_kind_is_counted_on_its_own() {
        let handshakes = Handshakes::default();
        assert_eq!(handshakes.counts(), (0, 0));
        handshakes.record(false);
        handshakes.record(true);
        handshakes.record(true);
        assert_eq!(handshakes.counts(), (1, 2));
    }

    #[test]
    fn a_display_has_nothing_until_it_has_connected() {
        assert_eq!(Handshakes::default().device(&device("kitchen")), None);
    }

    #[test]
    fn a_display_is_counted_by_how_its_own_connections_began_and_the_latest_is_remembered() {
        let handshakes = Handshakes::default();
        let kitchen = device("kitchen");
        handshakes.record_device(&kitchen, false, Duration::from_millis(900), at(0));
        handshakes.record_device(&kitchen, true, Duration::from_millis(250), at(10));
        handshakes.record_device(&kitchen, true, Duration::from_millis(240), at(20));
        let seen = handshakes.device(&kitchen).unwrap();
        assert_eq!((seen.full, seen.resumed), (1, 2));
        assert!(seen.last_resumed);
        assert_eq!(seen.last_handshake, Duration::from_millis(240));
        assert_eq!(seen.last_at, at(20));

        // Another display is its own.
        handshakes.record_device(&device("hall"), false, Duration::from_millis(1100), at(21));
        assert_eq!(handshakes.device(&kitchen).unwrap().full, 1);
        assert_eq!(handshakes.device(&device("hall")).unwrap().resumed, 0);
        // The server-wide counts are the connections' own, recorded separately.
        assert_eq!(handshakes.counts(), (0, 0));
    }

    #[test]
    fn a_display_that_falls_back_to_a_full_handshake_shows_as_such() {
        let handshakes = Handshakes::default();
        let kitchen = device("kitchen");
        handshakes.record_device(&kitchen, true, Duration::from_millis(200), at(0));
        handshakes.record_device(&kitchen, false, Duration::from_millis(1000), at(10));
        let seen = handshakes.device(&kitchen).unwrap();
        assert!(!seen.last_resumed);
        assert_eq!((seen.full, seen.resumed), (1, 1));
    }

    #[test]
    fn the_table_is_bounded_and_known_displays_still_count() {
        let handshakes = Handshakes::default();
        for i in 0..MAX_DEVICES + 10 {
            handshakes.record_device(&device(&format!("d{i}")), true, Duration::ZERO, at(0));
        }
        assert!(handshakes.device(&device("d0")).is_some());
        assert!(
            handshakes
                .device(&device(&format!("d{}", MAX_DEVICES + 5)))
                .is_none()
        );
        handshakes.record_device(&device("d0"), true, Duration::ZERO, at(1));
        assert_eq!(handshakes.device(&device("d0")).unwrap().resumed, 2);
    }
}

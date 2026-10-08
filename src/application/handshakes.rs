//! How many TLS connections began with a full handshake and how many resumed an earlier session, so the
//! owner can see whether displays are resuming (which saves them the certificate exchange and its signatures).

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Handshakes {
    full: AtomicU64,
    resumed: AtomicU64,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_counted_on_its_own() {
        let handshakes = Handshakes::default();
        assert_eq!(handshakes.counts(), (0, 0));
        handshakes.record(false);
        handshakes.record(true);
        handshakes.record(true);
        assert_eq!(handshakes.counts(), (1, 2));
    }
}

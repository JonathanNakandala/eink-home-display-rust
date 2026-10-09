//! Saying, in the log, which displays have stopped renewing their certificates.
//!
//! A display renews with a third of its certificate's life left, and one that has not (`renewal_overdue`) will lose its
//! certificate in a few weeks. `/status` and `/metrics` show it for anyone who looks or alerts on them; this is for the
//! owner who only reads the log. Each display is named once a day while it stays overdue, not on every check, and not
//! at all once it has renewed.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use crate::domain::models::device_id::DeviceId;

/// How long before a display that is still overdue is named again.
pub const REPEAT: Duration = Duration::hours(24);

#[derive(Debug, Default)]
pub struct RenewalWatch {
    told: HashMap<DeviceId, DateTime<Utc>>,
}

impl RenewalWatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Of the displays that are overdue now (with how long their certificates have left), those to say something about:
    /// each not named in the last `REPEAT`. A display that is no longer overdue is forgotten, so it is named again if it
    /// falls behind again.
    pub fn to_report(
        &mut self,
        overdue: Vec<(DeviceId, Duration)>,
        now: DateTime<Utc>,
    ) -> Vec<(DeviceId, Duration)> {
        self.told
            .retain(|device, _| overdue.iter().any(|(d, _)| d == device));
        overdue
            .into_iter()
            .filter(|(device, _)| {
                let due = self
                    .told
                    .get(device)
                    .is_none_or(|last| now - *last >= REPEAT);
                if due {
                    self.told.insert(device.clone(), now);
                }
                due
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn at(hours: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 0, 0, 0).unwrap() + Duration::hours(hours)
    }

    fn overdue(names: &[&str]) -> Vec<(DeviceId, Duration)> {
        names
            .iter()
            .map(|name| (device(name), Duration::days(20)))
            .collect()
    }

    #[test]
    fn a_display_that_is_overdue_is_named_at_once_and_then_once_a_day() {
        let mut watch = RenewalWatch::new();
        assert_eq!(watch.to_report(overdue(&["kitchen"]), at(0)).len(), 1);
        // The hourly checks that follow say nothing.
        for hour in 1..24 {
            assert!(watch.to_report(overdue(&["kitchen"]), at(hour)).is_empty());
        }
        assert_eq!(watch.to_report(overdue(&["kitchen"]), at(24)).len(), 1);
    }

    #[test]
    fn each_display_is_named_on_its_own_clock() {
        let mut watch = RenewalWatch::new();
        watch.to_report(overdue(&["kitchen"]), at(0));
        let told = watch.to_report(overdue(&["kitchen", "hall"]), at(5));
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].0, device("hall"));
    }

    #[test]
    fn a_display_that_renewed_and_falls_behind_again_is_named_again() {
        let mut watch = RenewalWatch::new();
        watch.to_report(overdue(&["kitchen"]), at(0));
        // It renewed: no longer overdue, so no longer remembered.
        assert!(watch.to_report(Vec::new(), at(2)).is_empty());
        assert_eq!(watch.to_report(overdue(&["kitchen"]), at(3)).len(), 1);
    }
}

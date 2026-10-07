use anyhow::Context;
use chrono_tz::Tz;

use crate::adapters::zone;
use crate::config::application::LocationConfig;

/// The zone everything is scheduled and shown in: the configured one, or else the host's.
///
/// The host's zone is a fallback and says so, because it is not a safe thing to rely on: a container or a
/// cloud machine is usually UTC whatever country the display is in, and then the schedule's hours, the
/// weekdays and the clock on the dashboard are all wrong without any error.
pub fn resolve_zone(location: &LocationConfig) -> anyhow::Result<Tz> {
    match location.zone()? {
        Some(zone) => {
            log::info!("Working in the timezone {}", zone.name());
            Ok(zone)
        }
        None => {
            let zone = zone::host();
            log::warn!(
                "No timezone is configured, so the host's is used: {}. Set `timezone = \"{}\"` under [location] \
                 so the schedule and the clock don't depend on the machine this runs on",
                zone.name(),
                zone.name()
            );
            Ok(zone)
        }
    }
}

/// The configured zone, checked, for a program that has no use for the fallback.
pub fn configured_zone(location: &LocationConfig) -> anyhow::Result<Option<Tz>> {
    location.zone().context("The [location] timezone is invalid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(timezone: Option<&str>) -> LocationConfig {
        LocationConfig { latitude: 0.0, longitude: 0.0, timezone: timezone.map(str::to_owned) }
    }

    #[test]
    fn a_configured_zone_is_used_whatever_the_host_says() {
        assert_eq!(resolve_zone(&location(Some("Pacific/Auckland"))).unwrap(), Tz::Pacific__Auckland);
        assert_eq!(resolve_zone(&location(Some("UTC"))).unwrap(), Tz::UTC);
    }

    #[test]
    fn without_one_the_host_zone_is_used() {
        assert_eq!(resolve_zone(&location(None)).unwrap(), zone::host());
    }

    #[test]
    fn a_zone_that_is_not_one_is_an_error_naming_it() {
        for bad in ["Europe/Londn", "BST", "", "+01:00", "Local"] {
            let error = format!("{:#}", resolve_zone(&location(Some(bad))).unwrap_err());
            assert!(error.contains("timezone") && error.contains(&format!("{bad:?}")), "{bad:?}: {error}");
        }
    }
}

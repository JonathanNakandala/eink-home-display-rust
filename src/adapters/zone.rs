//! The timezone of the machine this runs on, as an IANA name (`Europe/London`) found through the operating
//! system (`/etc/localtime` on Linux, the system settings on macOS and Windows). Used when none is configured.

use jiff::tz::TimeZone;

/// The name of `zone` for people to read and for the configuration: its IANA name, `UTC` for a zone that has none.
pub fn name(zone: &TimeZone) -> &str {
    zone.iana_name().unwrap_or("UTC")
}

/// The host's zone, or UTC if the system can't say or names one that isn't in the timezone database
/// (a container with no zone set reports UTC, which is right; a custom or empty setting is the case this catches).
pub fn host() -> TimeZone {
    match TimeZone::try_system() {
        Ok(zone) if zone.iana_name().is_some() => zone,
        Ok(_) => {
            tracing::warn!("The host's timezone isn't a known IANA zone; using UTC");
            TimeZone::UTC
        }
        Err(e) => {
            tracing::warn!(error = %e, "Couldn't tell the host's timezone; using UTC");
            TimeZone::UTC
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_zone_is_always_something() {
        // Whatever the machine says, a zone comes back and nothing panics.
        let zone = host();
        assert!(zone.iana_name().is_some());
    }
}

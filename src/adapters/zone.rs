//! The timezone of the machine this runs on, as an IANA name (`Europe/London`) found through the operating
//! system (`/etc/localtime` on Linux, the system settings on macOS and Windows). Used when none is configured.

use chrono_tz::Tz;

/// The host's zone, or UTC if the system can't say or names one that isn't in the timezone database
/// (a container with no zone set reports UTC, which is right; a custom or empty setting is the case this catches).
pub fn host() -> Tz {
    match iana_time_zone::get_timezone() {
        Ok(name) => name.parse().unwrap_or_else(|_| {
            log::warn!("The host's timezone {name:?} isn't a known IANA zone; using UTC");
            Tz::UTC
        }),
        Err(e) => {
            log::warn!("Couldn't tell the host's timezone ({e}); using UTC");
            Tz::UTC
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
        assert!(!zone.name().is_empty());
    }
}

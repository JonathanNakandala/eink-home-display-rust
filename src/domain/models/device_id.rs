//! The name a display goes by. The display sends it with every request, and everything the server
//! keeps about a display (its battery, when it was last seen, what it was last sent) is under it, so
//! requests from two displays are never mixed up: nothing is worked out afterwards from an address or
//! the order requests arrived in.
//!
//! The name comes from the network, so it is checked when it is made and a `DeviceId` that exists is a
//! good one. It is meant to be unique to the hardware (the firmware builds it from the chip's MAC
//! address, which survives flashing), and the server treats it as opaque: it is a key, not a place.

use std::fmt;

use serde::Serialize;
use thiserror::Error;

/// Long enough for a name with a MAC suffix (`reterminal-e1003-a1b2c3` is 23), short enough for a log line
/// and a metric label.
pub const MAX_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DeviceId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidDeviceId {
    #[error("a device name can't be empty")]
    Empty,
    #[error("a device name is at most {MAX_LEN} characters")]
    TooLong,
    #[error("a device name uses only letters, digits, '-', '_' and '.'")]
    BadCharacter,
    /// `.` and `..` mean "here" and "up" wherever a name is taken for a path, and a run of dots is read as neither
    /// by a person. Nothing here uses a name as a path, but a name should not be one anyone could mistake.
    #[error("a device name can't be only dots")]
    OnlyDots,
}

impl DeviceId {
    /// The name, trimmed, if it is a usable one. Plain characters only, so it is safe in a log line, a
    /// metric label and a URL without escaping.
    pub fn parse(name: &str) -> Result<Self, InvalidDeviceId> {
        let name = name.trim();
        if name.is_empty() {
            return Err(InvalidDeviceId::Empty);
        }
        if name.len() > MAX_LEN {
            return Err(InvalidDeviceId::TooLong);
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(InvalidDeviceId::BadCharacter);
        }
        if name.chars().all(|c| c == '.') {
            return Err(InvalidDeviceId::OnlyDots);
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for DeviceId {
    type Err = InvalidDeviceId;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::parse(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_short_name_is_kept_trimmed() {
        for name in [
            "kitchen",
            "reterminal-e1003-a1b2c3",
            "a.b_c-d",
            &"x".repeat(MAX_LEN),
        ] {
            assert_eq!(DeviceId::parse(name).unwrap().as_str(), name);
        }
        assert_eq!(DeviceId::parse("  kitchen\n").unwrap().as_str(), "kitchen");
    }

    #[test]
    fn anything_else_is_refused_and_says_why() {
        assert_eq!(DeviceId::parse(""), Err(InvalidDeviceId::Empty));
        assert_eq!(DeviceId::parse("   "), Err(InvalidDeviceId::Empty));
        assert_eq!(
            DeviceId::parse(&"x".repeat(MAX_LEN + 1)),
            Err(InvalidDeviceId::TooLong)
        );
        for bad in [
            "has space",
            "quote\"d",
            "new\nline",
            "a/b",
            "emoji🔋",
            "a;b",
            "{x}",
        ] {
            assert_eq!(
                DeviceId::parse(bad),
                Err(InvalidDeviceId::BadCharacter),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn it_reads_and_writes_as_a_plain_string() {
        let id: DeviceId = "kitchen".parse().unwrap();
        assert_eq!(id.to_string(), "kitchen");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"kitchen\"");
    }

    #[test]
    fn a_name_of_only_dots_is_refused_and_dots_among_other_characters_are_not() {
        for dots in [".", "..", "...", "  ..  "] {
            assert_eq!(
                DeviceId::parse(dots),
                Err(InvalidDeviceId::OnlyDots),
                "{dots:?}"
            );
        }
        for fine in [".a", "a.", "a.b", "a..b", "a-.", "e1003.local", "1.2"] {
            assert!(DeviceId::parse(fine).is_ok(), "{fine:?}");
        }
    }
}

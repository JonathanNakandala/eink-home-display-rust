//! The keys the server uses to ask other services for data, and what a usable one looks like.
//!
//! There is a fixed set of them, one for each service that wants one. A key is a secret: nothing here prints one, and the
//! rules say how long a key is or what it may hold, never what is wrong with the particular one given (a message that
//! repeated a key would be a way for it to reach a log).

use std::fmt;

use thiserror::Error;

/// A service the server holds a key for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialName {
    /// The OpenWeather API key, for the weather.
    OpenWeather,
    /// The Consumer Key of the raildata.org.uk "Live Arrival and Departure Boards" subscription, for national rail departures.
    OpenLdbws,
    /// The TfL Unified API application key, for bus and tube departures. TfL answers without one, but slowly.
    Tfl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownCredential(String);

impl fmt::Display for UnknownCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = CredentialName::ALL.map(CredentialName::as_str).join(", ");
        write!(f, "no key is called {:?}; the keys are {names}", self.0)
    }
}

impl std::error::Error for UnknownCredential {}

/// Why a key cannot be used as given. Never says what the key was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InvalidCredential {
    #[error("the key is empty")]
    Empty,
    #[error("the key is {length} characters; the most there can be is {max}")]
    TooLong { length: usize, max: usize },
    #[error("the key must be {expected} characters, not {length}")]
    WrongLength { expected: usize, length: usize },
    #[error("the key holds a space, a line break or a character that is not plain text")]
    BadCharacter,
}

impl CredentialName {
    pub const ALL: [CredentialName; 3] = [Self::OpenWeather, Self::OpenLdbws, Self::Tfl];

    /// The name in a command, a path and a file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenWeather => "open-weather",
            Self::OpenLdbws => "open-ldbws",
            Self::Tfl => "tfl",
        }
    }

    pub fn parse(name: &str) -> Result<Self, UnknownCredential> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == name.trim())
            .ok_or_else(|| UnknownCredential(name.trim().to_owned()))
    }

    /// What the key is for, in a few words.
    pub fn describe(self) -> &'static str {
        match self {
            Self::OpenWeather => "OpenWeather API key (weather)",
            Self::OpenLdbws => "raildata.org.uk Consumer Key (National Rail departures)",
            Self::Tfl => "TfL application key (bus and tube departures); optional",
        }
    }

    /// Whether the service answers without a key, so that having none is a choice and not a gap.
    pub fn is_optional(self) -> bool {
        matches!(self, Self::Tfl)
    }

    /// Whether `key` is one this service could have given: the right length where there is one, and plain text that can
    /// travel in a request. It cannot say the service will accept it.
    pub fn check(self, key: &str) -> Result<(), InvalidCredential> {
        if key.is_empty() {
            return Err(InvalidCredential::Empty);
        }
        let length = key.chars().count();
        if !key.chars().all(|c| c.is_ascii_graphic()) {
            return Err(InvalidCredential::BadCharacter);
        }
        match self {
            Self::OpenWeather if length != 32 => Err(InvalidCredential::WrongLength {
                expected: 32,
                length,
            }),
            _ if length > MAX_LEN => Err(InvalidCredential::TooLong {
                length,
                max: MAX_LEN,
            }),
            _ => Ok(()),
        }
    }
}

/// More than any service's key is.
pub const MAX_LEN: usize = 256;

impl fmt::Display for CredentialName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_found_by_its_name_and_by_no_other() {
        for name in CredentialName::ALL {
            assert_eq!(CredentialName::parse(name.as_str()), Ok(name));
            assert_eq!(CredentialName::parse(&format!(" {name} ")), Ok(name));
        }
        let error = CredentialName::parse("weather").unwrap_err().to_string();
        assert!(
            error.contains("open-weather") && error.contains("tfl"),
            "{error}"
        );
    }

    #[test]
    fn an_openweather_key_is_32_characters() {
        let key = "0123456789abcdef0123456789abcdef";
        assert_eq!(CredentialName::OpenWeather.check(key), Ok(()));
        assert_eq!(
            CredentialName::OpenWeather.check(&key[1..]),
            Err(InvalidCredential::WrongLength {
                expected: 32,
                length: 31
            })
        );
    }

    #[test]
    fn the_other_keys_have_no_set_length_but_are_plain_and_not_huge() {
        for name in [CredentialName::OpenLdbws, CredentialName::Tfl] {
            assert_eq!(name.check("a"), Ok(()));
            assert_eq!(name.check(&"k".repeat(MAX_LEN)), Ok(()));
            assert!(matches!(
                name.check(&"k".repeat(MAX_LEN + 1)),
                Err(InvalidCredential::TooLong { .. })
            ));
        }
    }

    #[test]
    fn a_key_with_a_space_a_line_break_or_a_non_ascii_character_is_refused() {
        for key in ["a b", "ab\n", "ab\r\n", " ab", "ab\t", "clé", "ab\u{0}"] {
            assert_eq!(
                CredentialName::Tfl.check(key),
                Err(InvalidCredential::BadCharacter),
                "{key:?}"
            );
        }
        assert_eq!(CredentialName::Tfl.check(""), Err(InvalidCredential::Empty));
    }

    #[test]
    fn what_is_said_of_a_bad_key_never_repeats_it() {
        let key = "secret value with spaces";
        for name in CredentialName::ALL {
            if let Err(e) = name.check(key) {
                assert!(!e.to_string().contains("secret"), "{e}");
            }
        }
    }
}

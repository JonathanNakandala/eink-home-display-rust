//! Secrets in the configuration (API keys, passwords) are held as `SecretString`: its `Debug` prints
//! `[REDACTED]`, so logging the config, or an error that holds it, can't leak one; it has no `Display`;
//! and it is wiped from memory when dropped. Reading one takes an explicit `expose_secret()`, which
//! is easy to find and to review.
//!
//! The configuration types also derive `Serialize`, for writing the example file and in tests, and a
//! `SecretString` deliberately can't be serialized. These helpers opt a field in, so that the one
//! place that writes a config file can write its keys. Nothing at run time serializes the config.

use secrecy::{ExposeSecret, SecretString};
use serde::Serializer;

pub fn serialize<S: Serializer>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

pub fn serialize_option<S: Serializer>(secret: &Option<SecretString>, serializer: S) -> Result<S::Ok, S::Error> {
    match secret {
        Some(secret) => serialize(secret, serializer),
        None => serializer.serialize_none(),
    }
}

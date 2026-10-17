//! Where a service that needs a key gets it from.
//!
//! The key is asked for each time it is used, not copied when the service is made, so that one that the owner sets or
//! takes away while the server runs is the one used from the next request on.

use std::collections::BTreeMap;

use secrecy::SecretString;

use crate::domain::models::credential::CredentialName;

pub trait CredentialSource: Send + Sync {
    /// The key for `name` now, if there is one.
    fn key(&self, name: CredentialName) -> Option<SecretString>;
}

/// Keys that never change, for a program that runs once with the keys it was given, and for tests.
#[derive(Default)]
pub struct FixedCredentials(BTreeMap<CredentialName, SecretString>);

impl FixedCredentials {
    pub fn none() -> Self {
        Self::default()
    }

    /// With `key` as the key for `name`.
    pub fn with(mut self, name: CredentialName, key: impl Into<SecretString>) -> Self {
        self.0.insert(name, key.into());
        self
    }
}

impl CredentialSource for FixedCredentials {
    fn key(&self, name: CredentialName) -> Option<SecretString> {
        self.0.get(&name).cloned()
    }
}

//! The keys the owner asked the server to keep across a restart.
//!
//! A key is only here when it was set to be remembered. Without that it lives in the server's memory alone and is gone
//! when the server stops.

use std::collections::BTreeMap;

use secrecy::SecretString;

use crate::domain::models::credential::CredentialName;

#[async_trait::async_trait]
pub trait RememberedCredentials: Send + Sync {
    /// Every key kept.
    async fn load(&self) -> anyhow::Result<BTreeMap<CredentialName, SecretString>>;

    /// Keeps `key` for `name`, in place of any kept before. Once this returns the key survives a restart.
    async fn remember(&self, name: CredentialName, key: &SecretString) -> anyhow::Result<()>;

    /// Stops keeping the key for `name`. Having none kept is not an error.
    async fn forget(&self, name: CredentialName) -> anyhow::Result<()>;
}

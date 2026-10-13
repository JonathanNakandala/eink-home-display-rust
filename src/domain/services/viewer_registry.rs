//! Where the viewers are kept, so that they outlast a restart.

use crate::domain::models::viewer::{SecretHash, Viewer, ViewerId};

#[async_trait::async_trait]
pub trait ViewerRegistry: Send + Sync {
    async fn get(&self, id: &ViewerId) -> anyhow::Result<Option<Viewer>>;

    /// Adds `viewer`, or replaces the one with its id.
    async fn put(&self, viewer: &Viewer) -> anyhow::Result<()>;

    /// Forgets a viewer; nothing there is not an error.
    async fn remove(&self, id: &ViewerId) -> anyhow::Result<()>;

    /// Every viewer, oldest first.
    async fn all(&self) -> anyhow::Result<Vec<Viewer>>;

    /// The viewer whose link has this code, if any.
    async fn find_by_code(&self, code: &SecretHash) -> anyhow::Result<Option<Viewer>>;

    /// The viewer that has this token, if any.
    async fn find_by_token(&self, token: &SecretHash) -> anyhow::Result<Option<Viewer>>;

    /// The viewer whose current refresh token this is, or whose one before it, which is how a refresh token shown again is
    /// told from one never issued.
    async fn find_by_refresh(&self, token: &SecretHash) -> anyhow::Result<Option<Viewer>>;
}

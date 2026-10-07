//! Where the pairings are kept, so they outlast a restart.

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::Pairing;

#[async_trait::async_trait]
pub trait PairingStore: Send + Sync {
    async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<Pairing>>;

    /// Adds or replaces the pairing of `pairing.device`.
    async fn put(&self, pairing: &Pairing) -> anyhow::Result<()>;

    /// Forgets a display; it is as if it had never asked.
    async fn remove(&self, device: &DeviceId) -> anyhow::Result<()>;

    /// Every display, in name order.
    async fn all(&self) -> anyhow::Result<Vec<Pairing>>;
}

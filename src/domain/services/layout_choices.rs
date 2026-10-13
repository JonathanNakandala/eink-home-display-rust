//! Which layout the owner chose for each display, so the choice outlasts a restart.

use std::collections::BTreeMap;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::layout::LayoutName;

#[async_trait::async_trait]
pub trait LayoutChoices: Send + Sync {
    /// The layout chosen for `device`, if one was.
    async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<LayoutName>>;

    /// Chooses `layout` for `device`, in place of any earlier choice.
    async fn set(&self, device: &DeviceId, layout: LayoutName) -> anyhow::Result<()>;

    /// Takes the choice for `device` away, so it gets the layout for the time of day. Nothing to take away is not an error.
    async fn clear(&self, device: &DeviceId) -> anyhow::Result<()>;

    /// Every choice, by display.
    async fn all(&self) -> anyhow::Result<BTreeMap<DeviceId, LayoutName>>;
}

use std::path::PathBuf;
use crate::domain::models::GlanceData;

#[allow(async_fn_in_trait)]
pub trait DisplayImageGenerator {
    async fn generate(&self, data: GlanceData) -> anyhow::Result<PathBuf>;
}
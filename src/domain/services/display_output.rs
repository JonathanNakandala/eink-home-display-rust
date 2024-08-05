use crate::domain::models::OutputResult;
use async_trait::async_trait;
#[async_trait]
pub trait DisplayOutput {
    async fn generate(&self, data: Vec<u8>) -> anyhow::Result<OutputResult>;
}

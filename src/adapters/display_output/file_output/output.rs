use crate::domain::{models::OutputResult, services::display_output::DisplayOutput};
use async_trait::async_trait;
#[derive(derive_new::new)]
pub struct FileOutputAdapter {}
#[async_trait]
impl DisplayOutput for FileOutputAdapter {
    async fn generate(&self, data: Vec<u8>) -> anyhow::Result<OutputResult> {
        log::debug!("Time to Generate!");

        Ok(OutputResult {
            output_path: "/tmp/".into(),
        })
    }
}

use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::domain::models::image::ImageData;
use crate::domain::services::image_repository::ImageRepository;

#[derive(derive_new::new)]
pub struct FileStoreImageRepository {
    save_dir: PathBuf,
}

#[async_trait::async_trait]
impl ImageRepository for FileStoreImageRepository {
    async fn store(&self, image_data: &ImageData) -> anyhow::Result<()> {
        // Created on every store, so it also comes back if it is deleted while running.
        tokio::fs::create_dir_all(&self.save_dir)
            .await
            .with_context(|| format!("Failed to create {}", self.save_dir.display()))?;
        let output_path = self.save_dir.join("screenshot.png");

        let mut file = tokio::fs::File::create(&output_path)
            .await
            .with_context(|| format!("Failed to create {}", output_path.display()))?;
        file.write_all(&image_data.data).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_a_missing_save_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let save_dir = tmp.path().join("nested").join("output");

        FileStoreImageRepository::new(save_dir.clone())
            .store(&ImageData::new(vec![1, 2, 3]))
            .await
            .unwrap();

        assert_eq!(std::fs::read(save_dir.join("screenshot.png")).unwrap(), vec![1, 2, 3]);
    }
}

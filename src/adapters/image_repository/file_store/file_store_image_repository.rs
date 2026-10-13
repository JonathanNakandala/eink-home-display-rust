use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::image_repository::ImageRepository;

#[derive(derive_new::new)]
pub struct FileStoreImageRepository {
    save_dir: PathBuf,
}

/// One file for each layout, side by side.
fn file_name(layout: &LayoutName) -> String {
    format!("screenshot-{layout}.png")
}

#[async_trait::async_trait]
impl ImageRepository for FileStoreImageRepository {
    async fn store(&self, layout: &LayoutName, image_data: &ImageData) -> anyhow::Result<()> {
        // Created on every store, so it also comes back if it is deleted while running.
        tokio::fs::create_dir_all(&self.save_dir)
            .await
            .with_context(|| format!("Failed to create {}", self.save_dir.display()))?;
        let output_path = self.save_dir.join(file_name(layout));

        let mut file = tokio::fs::File::create(&output_path)
            .await
            .with_context(|| format!("Failed to create {}", output_path.display()))?;
        file.write_all(&image_data.data).await?;
        file.flush().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dashboard() -> LayoutName {
        LayoutName::parse("dashboard").unwrap()
    }

    #[tokio::test]
    async fn creates_a_missing_save_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let save_dir = tmp.path().join("nested").join("output");

        FileStoreImageRepository::new(save_dir.clone())
            .store(&dashboard(), &ImageData::new(vec![1, 2, 3]))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(save_dir.join("screenshot-dashboard.png")).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn each_layout_is_kept_in_a_file_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let repository = FileStoreImageRepository::new(tmp.path().to_path_buf());
        repository
            .store(&dashboard(), &ImageData::new(vec![1]))
            .await
            .unwrap();
        repository
            .store(
                &LayoutName::parse("departures").unwrap(),
                &ImageData::new(vec![2]),
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(tmp.path().join("screenshot-dashboard.png")).unwrap(),
            [1]
        );
        assert_eq!(
            std::fs::read(tmp.path().join("screenshot-departures.png")).unwrap(),
            [2]
        );
    }
}

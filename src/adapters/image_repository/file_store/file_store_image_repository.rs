use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::domain::models::image::ImageData;
use crate::domain::models::layout::{DEFAULT_LAYOUT, LayoutName};
use crate::domain::services::image_repository::ImageRepository;

#[derive(derive_new::new)]
pub struct FileStoreImageRepository {
    save_dir: PathBuf,
}

/// `screenshot.png` for the default layout, as it has always been, and `screenshot-<layout>.png` for the others.
fn file_name(layout: &LayoutName) -> String {
    if layout.as_str() == DEFAULT_LAYOUT {
        "screenshot.png".to_owned()
    } else {
        format!("screenshot-{layout}.png")
    }
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
        LayoutName::parse(DEFAULT_LAYOUT).unwrap()
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
            std::fs::read(save_dir.join("screenshot.png")).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn another_layout_is_kept_beside_the_first() {
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
            std::fs::read(tmp.path().join("screenshot.png")).unwrap(),
            [1]
        );
        assert_eq!(
            std::fs::read(tmp.path().join("screenshot-departures.png")).unwrap(),
            [2]
        );
    }
}

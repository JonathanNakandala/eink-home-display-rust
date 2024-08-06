use std::path::PathBuf;
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
        let output_path = self.save_dir.join("one_bit_screenshot.png");

        let mut file = tokio::fs::File::create(output_path).await?;
        file.write_all(&*image_data.data).await?;
        
        Ok(())
    }
}

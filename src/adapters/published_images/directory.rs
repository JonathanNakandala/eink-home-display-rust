use std::path::{Path, PathBuf};

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Local};

use crate::domain::models::display::ImageFormat;
use crate::domain::services::published_images::PublishedImages;

/// Publishes images as `image.<ext>` files in a directory. The file's modification time is when
/// it was published.
pub struct DirectoryImages {
    directory: PathBuf,
}

impl DirectoryImages {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self { directory: directory.into() }
    }

    fn path(&self, format: ImageFormat) -> PathBuf {
        self.directory.join(format!("image.{}", format.extension()))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[async_trait]
impl PublishedImages for DirectoryImages {
    /// Written beside the target and renamed into place, so a download in progress never sees
    /// half a file.
    async fn publish(&self, format: ImageFormat, bytes: &[u8]) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .with_context(|| format!("Failed to create {}", self.directory.display()))?;
        let target = self.path(format);
        let staging = target.with_extension(format!("{}.tmp", format.extension()));
        tokio::fs::write(&staging, bytes)
            .await
            .with_context(|| format!("Failed to write {}", staging.display()))?;
        tokio::fs::rename(&staging, &target)
            .await
            .with_context(|| format!("Failed to move the image into {}", target.display()))?;
        Ok(())
    }

    async fn published_at(&self, format: ImageFormat) -> anyhow::Result<Option<DateTime<Local>>> {
        let path = self.path(format);
        match tokio::fs::metadata(&path).await.and_then(|metadata| metadata.modified()) {
            Ok(modified) => Ok(Some(DateTime::<Local>::from(modified))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("Failed to inspect {}", path.display())),
        }
    }

    async fn read(&self, format: ImageFormat) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.path(format);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_reads_and_dates_each_format() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path().join("out"));
        assert!(images.published_at(ImageFormat::Bmp).await.unwrap().is_none());
        assert!(images.read(ImageFormat::Bmp).await.unwrap().is_none());

        images.publish(ImageFormat::Bmp, &[1, 2, 3]).await.unwrap();
        assert_eq!(images.read(ImageFormat::Bmp).await.unwrap().unwrap(), [1, 2, 3]);
        let at = images.published_at(ImageFormat::Bmp).await.unwrap().unwrap();
        assert!((Local::now() - at).num_seconds().abs() < 5);
        // Another format is its own file.
        assert!(images.published_at(ImageFormat::Png).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_new_publish_replaces_the_image_and_leaves_no_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        images.publish(ImageFormat::Bmp, &[1]).await.unwrap();
        images.publish(ImageFormat::Bmp, &[9]).await.unwrap();

        assert_eq!(images.read(ImageFormat::Bmp).await.unwrap().unwrap(), [9]);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}

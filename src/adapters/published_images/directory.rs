use std::path::PathBuf;

use anyhow::Context;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::published_images::PublishedImages;

/// Publishes images as `<layout>/image.<ext>` files in a directory. The file's modification time is when
/// it was published.
pub struct DirectoryImages {
    directory: PathBuf,
}

impl DirectoryImages {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// A layout's name is only letters, digits and hyphens (see `LayoutName`), so it can't point elsewhere.
    fn folder(&self, layout: &LayoutName) -> PathBuf {
        self.directory.join(layout.as_str())
    }

    fn path(&self, layout: &LayoutName, format: ImageFormat) -> PathBuf {
        self.folder(layout)
            .join(format!("image.{}", format.extension()))
    }
}

#[async_trait]
impl PublishedImages for DirectoryImages {
    /// Written beside the target and renamed into place, so a download in progress never sees
    /// half a file.
    async fn publish(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let folder = self.folder(layout);
        tokio::fs::create_dir_all(&folder)
            .await
            .with_context(|| format!("Failed to create {}", folder.display()))?;
        let target = self.path(layout, format);
        let staging = target.with_extension(format!("{}.tmp", format.extension()));
        tokio::fs::write(&staging, bytes)
            .await
            .with_context(|| format!("Failed to write {}", staging.display()))?;
        tokio::fs::rename(&staging, &target)
            .await
            .with_context(|| format!("Failed to move the image into {}", target.display()))?;
        Ok(())
    }

    async fn published_at(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<DateTime<Utc>>> {
        let path = self.path(layout, format);
        match tokio::fs::metadata(&path)
            .await
            .and_then(|metadata| metadata.modified())
        {
            Ok(modified) => Ok(Some(DateTime::<Utc>::from(modified))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("Failed to inspect {}", path.display())),
        }
    }

    async fn read(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let path = self.path(layout, format);
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

    fn dash() -> LayoutName {
        LayoutName::parse("dashboard").unwrap()
    }

    #[tokio::test]
    async fn publishes_reads_and_dates_each_format() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path().join("out"));
        assert!(
            images
                .published_at(&dash(), ImageFormat::Bmp)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            images
                .read(&dash(), ImageFormat::Bmp)
                .await
                .unwrap()
                .is_none()
        );

        images
            .publish(&dash(), ImageFormat::Bmp, &[1, 2, 3])
            .await
            .unwrap();
        assert_eq!(
            images
                .read(&dash(), ImageFormat::Bmp)
                .await
                .unwrap()
                .unwrap(),
            [1, 2, 3]
        );
        let at = images
            .published_at(&dash(), ImageFormat::Bmp)
            .await
            .unwrap()
            .unwrap();
        assert!((Utc::now() - at).num_seconds().abs() < 5);
        // Another format is its own file.
        assert!(
            images
                .published_at(&dash(), ImageFormat::Png)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_new_publish_replaces_the_image_and_leaves_no_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        images
            .publish(&dash(), ImageFormat::Bmp, &[1])
            .await
            .unwrap();
        images
            .publish(&dash(), ImageFormat::Bmp, &[9])
            .await
            .unwrap();

        assert_eq!(
            images
                .read(&dash(), ImageFormat::Bmp)
                .await
                .unwrap()
                .unwrap(),
            [9]
        );
        assert_eq!(
            std::fs::read_dir(tmp.path().join("dashboard"))
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn each_layout_has_its_own_images() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        let other = LayoutName::parse("departures").unwrap();
        images
            .publish(&dash(), ImageFormat::Bmp, &[1])
            .await
            .unwrap();

        assert!(
            images
                .published_at(&other, ImageFormat::Bmp)
                .await
                .unwrap()
                .is_none()
        );
        images
            .publish(&other, ImageFormat::Bmp, &[2])
            .await
            .unwrap();
        assert_eq!(
            images
                .read(&dash(), ImageFormat::Bmp)
                .await
                .unwrap()
                .unwrap(),
            [1]
        );
        assert_eq!(
            images
                .read(&other, ImageFormat::Bmp)
                .await
                .unwrap()
                .unwrap(),
            [2]
        );
    }
}

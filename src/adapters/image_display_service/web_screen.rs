//! A web browser as an output: a picture for the page at `/view`, drawn at the page's own size in full colour.
//!
//! The panels get what they can show: the reTerminal 16 greys at its own size, in several formats. A tablet on a wall can
//! show colour at its own resolution, and a PNG is the one format every browser has, so this keeps the render as it came
//! from Chrome (no reduction of its colours, no dithering) and publishes just that, in a folder of its own.

use std::io::Cursor;
use std::sync::Arc;

use anyhow::{Context, bail};
use async_trait::async_trait;

use crate::domain::models::display::{DisplayProfile, ImageFormat, Palette};
use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::image_display_service::ImageDisplayService;
use crate::domain::services::published_images::PublishedImages;

pub struct WebScreenAdapter {
    width: u32,
    height: u32,
    images: Arc<dyn PublishedImages>,
}

impl WebScreenAdapter {
    pub fn new(width: u32, height: u32, images: Arc<dyn PublishedImages>) -> Self {
        Self {
            width,
            height,
            images,
        }
    }
}

#[async_trait]
impl ImageDisplayService for WebScreenAdapter {
    fn name(&self) -> &'static str {
        "web"
    }

    fn profile(&self) -> DisplayProfile {
        DisplayProfile {
            width: self.width,
            height: self.height,
            palette: Palette::Rgb,
        }
    }

    /// The page lets the viewer choose among them.
    fn wants_every_layout(&self) -> bool {
        true
    }

    async fn display(&self, layout: &LayoutName, data: &ImageData) -> anyhow::Result<()> {
        let (width, height) = image::ImageReader::new(Cursor::new(&data.data))
            .with_guessed_format()
            .context("Failed to read the picture for the page")?
            .into_dimensions()
            .context("The picture for the page is not an image")?;
        if (width, height) != (self.width, self.height) {
            bail!(
                "The picture is {width}x{height}, but the page is drawn for {}x{}",
                self.width,
                self.height
            );
        }
        self.images
            .publish(layout, ImageFormat::Png, &data.data)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::adapters::published_images::DirectoryImages;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn dashboard() -> LayoutName {
        LayoutName::parse("dashboard").unwrap()
    }

    #[test]
    fn it_is_the_page_at_its_own_size_in_full_colour_with_every_layout_to_choose_from() {
        let adapter = WebScreenAdapter::new(
            1920,
            1080,
            Arc::new(DirectoryImages::new(std::env::temp_dir())),
        );

        let profile = adapter.profile();

        assert_eq!((profile.width, profile.height), (1920, 1080));
        assert_eq!(profile.palette, Palette::Rgb);
        assert!(adapter.wants_every_layout());
        assert_eq!(adapter.name(), "web");
    }

    #[tokio::test]
    async fn the_render_is_published_as_it_came_keeping_its_colours_and_only_as_a_png() {
        let tmp = tempfile::tempdir().unwrap();
        let images = Arc::new(DirectoryImages::new(tmp.path()));
        let adapter = WebScreenAdapter::new(64, 36, images.clone());
        let rendered = png(64, 36);

        adapter
            .display(&dashboard(), &ImageData::new(rendered.clone()))
            .await
            .unwrap();

        // Byte for byte: no reduction to greys, no re-encoding.
        let kept = images
            .read(&dashboard(), ImageFormat::Png)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept, rendered);
        let decoded = image::load_from_memory(&kept).unwrap().to_rgb8();
        assert_eq!(decoded.get_pixel(0, 0).0, [200, 30, 30]);
        // Nothing for a panel's formats: the page does not use them.
        for format in [ImageFormat::Bmp, ImageFormat::Qoi] {
            assert!(images.read(&dashboard(), format).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn a_picture_of_another_size_or_not_a_picture_is_refused_and_publishes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let images = Arc::new(DirectoryImages::new(tmp.path()));
        let adapter = WebScreenAdapter::new(64, 36, images.clone());

        let wrong_size = adapter
            .display(&dashboard(), &ImageData::new(png(10, 10)))
            .await
            .unwrap_err();
        let not_an_image = adapter
            .display(&dashboard(), &ImageData::new(b"not a png".to_vec()))
            .await
            .unwrap_err();

        assert!(
            format!("{wrong_size:#}").contains("10x10"),
            "{wrong_size:#}"
        );
        assert!(
            format!("{not_an_image:#}").contains("not an image"),
            "{not_an_image:#}"
        );
        assert!(
            images
                .published_at(&dashboard(), ImageFormat::Png)
                .await
                .unwrap()
                .is_none()
        );
    }
}

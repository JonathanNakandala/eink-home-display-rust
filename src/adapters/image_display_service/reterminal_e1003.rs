use std::io::Cursor;
use std::sync::Arc;

use anyhow::{Context, bail};
use async_trait::async_trait;
use image::codecs::bmp::BmpEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::qoi::QoiEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::adapters::image_display_service::quantise::quantise_grey;
use crate::domain::models::display::{DisplayProfile, Dither, ImageFormat, Palette};
use crate::domain::models::image::ImageData;
use crate::domain::services::image_display_service::ImageDisplayService;
use crate::domain::services::published_images::PublishedImages;

const WIDTH: u32 = 1872;
const HEIGHT: u32 = 1404;
const GREY_LEVELS: u8 = 16;

/// Seeed reTerminal E1003: 1872x1404, 16 greys. It can't be driven from here, so the
/// image is published for the device to download (see `adapters::image_server`)
/// when it wakes. Every format the server can send is published, so the device can pick one
/// with its `Accept` header; `format` is the one sent when it has no preference.
#[derive(derive_new::new)]
pub struct ReTerminalE1003Adapter {
    dither: Dither,
    format: ImageFormat,
    images: Arc<dyn PublishedImages>,
}

#[async_trait]
impl ImageDisplayService for ReTerminalE1003Adapter {
    fn profile(&self) -> DisplayProfile {
        DisplayProfile {
            width: WIDTH,
            height: HEIGHT,
            palette: Palette::Grey(GREY_LEVELS),
        }
    }

    async fn display(&self, data: &ImageData) -> anyhow::Result<()> {
        let encoded = data.data.clone();
        let (dither, preferred) = (self.dither, self.format);
        // Dithering a 2.6 megapixel image is too much work to do on the async threads.
        let files =
            tokio::task::spawn_blocking(move || encode_all(&encoded, dither, preferred)).await??;
        for (format, bytes) in files {
            self.images.publish(format, &bytes).await?;
        }
        Ok(())
    }
}

/// Every format of the picture, with `preferred` last: its file's date is the render's version, so
/// when it changes the others are already in place.
fn encode_all(
    encoded: &[u8],
    dither: Dither,
    preferred: ImageFormat,
) -> anyhow::Result<Vec<(ImageFormat, Vec<u8>)>> {
    let grey = quantise_for_panel(encoded, dither)?;
    let mut formats: Vec<ImageFormat> = ImageFormat::ALL
        .into_iter()
        .filter(|format| *format != preferred)
        .collect();
    formats.push(preferred);
    formats
        .into_iter()
        .map(|format| Ok((format, encode(&grey, format)?)))
        .collect()
}

/// Decodes the rendered image and reduces it to the panel's 16 greys, so what the device
/// draws matches what was previewed.
fn quantise_for_panel(encoded: &[u8], dither: Dither) -> anyhow::Result<image::GrayImage> {
    let image = image::load_from_memory(encoded).context("Failed to decode display image")?;
    if (image.width(), image.height()) != (WIDTH, HEIGHT) {
        bail!(
            "Image is {}x{}, but the panel is {WIDTH}x{HEIGHT}",
            image.width(),
            image.height()
        );
    }
    Ok(quantise_grey(&image.to_luma8(), GREY_LEVELS, dither))
}

fn encode(grey: &image::GrayImage, format: ImageFormat) -> anyhow::Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Bmp => {
            BmpEncoder::new(&mut out).encode(grey.as_raw(), WIDTH, HEIGHT, ExtendedColorType::L8)
        }
        ImageFormat::Png => PngEncoder::new(&mut out).write_image(
            grey.as_raw(),
            WIDTH,
            HEIGHT,
            ExtendedColorType::L8,
        ),
        // QOI has only RGB and RGBA, so each grey becomes a pixel with three equal channels.
        ImageFormat::Qoi => {
            let rgb: Vec<u8> = grey
                .as_raw()
                .iter()
                .flat_map(|&level| [level, level, level])
                .collect();
            QoiEncoder::new(&mut out).write_image(&rgb, WIDTH, HEIGHT, ExtendedColorType::Rgb8)
        }
    }
    .context("Failed to encode the display image")?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use image::{GrayImage, Luma};

    use super::*;
    use crate::adapters::published_images::DirectoryImages;

    fn encode_for_panel(
        encoded: &[u8],
        dither: Dither,
        format: ImageFormat,
    ) -> anyhow::Result<Vec<u8>> {
        encode(&quantise_for_panel(encoded, dither)?, format)
    }

    fn rendered() -> Vec<u8> {
        let img = GrayImage::from_fn(WIDTH, HEIGHT, |x, _| Luma([(x * 255 / WIDTH) as u8]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn every_format_decodes_back_to_sixteen_levels() {
        for format in ImageFormat::ALL {
            let bytes = encode_for_panel(&rendered(), Dither::None, format).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().to_luma8();

            assert_eq!(decoded.dimensions(), (WIDTH, HEIGHT));
            let mut levels: Vec<u8> = decoded.pixels().map(|p| p[0]).collect();
            levels.sort_unstable();
            levels.dedup();
            assert_eq!(levels.len(), GREY_LEVELS as usize, "{format:?}");
            assert!(levels.iter().all(|l| l % 17 == 0), "{format:?}: {levels:?}");
        }
    }

    #[test]
    fn png_is_far_smaller_than_bmp() {
        let bmp = encode_for_panel(&rendered(), Dither::None, ImageFormat::Bmp).unwrap();
        let png = encode_for_panel(&rendered(), Dither::None, ImageFormat::Png).unwrap();
        assert!(
            png.len() * 10 < bmp.len(),
            "png {} bmp {}",
            png.len(),
            bmp.len()
        );
    }

    #[test]
    fn qoi_is_far_smaller_than_bmp_and_the_same_picture() {
        let bmp = encode_for_panel(&rendered(), Dither::None, ImageFormat::Bmp).unwrap();
        let qoi = encode_for_panel(&rendered(), Dither::None, ImageFormat::Qoi).unwrap();
        assert!(
            qoi.len() * 4 < bmp.len(),
            "qoi {} bmp {}",
            qoi.len(),
            bmp.len()
        );
        // The same pixels as BMP, not merely the same number of levels.
        let decode = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_luma8();
        assert_eq!(decode(&qoi), decode(&bmp));
    }

    #[test]
    fn rejects_an_image_of_the_wrong_size() {
        let img = GrayImage::new(10, 10);
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        assert!(encode_for_panel(&out.into_inner(), Dither::None, ImageFormat::Bmp).is_err());
    }

    #[tokio::test]
    async fn display_publishes_into_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let images = Arc::new(DirectoryImages::new(tmp.path().join("out")));
        let adapter = ReTerminalE1003Adapter::new(Dither::None, ImageFormat::Png, images);

        adapter.display(&ImageData::new(rendered())).await.unwrap();

        // Every format is published, so the display can ask for any of them.
        for file in ["image.png", "image.bmp", "image.qoi"] {
            let published = std::fs::read(tmp.path().join("out").join(file)).unwrap();
            assert_eq!(
                image::load_from_memory(&published).unwrap().width(),
                WIDTH,
                "{file}"
            );
        }
    }
}

use anyhow::{Context, bail};
use image::DynamicImage;

use super::eink_driver::{BUFFER_SIZE, HEIGHT, WIDTH};
use crate::adapters::image_display_service::quantise::quantise_grey;
use crate::domain::models::display::Dither;

/// Decode an encoded image (PNG) into the panel's frame format: 1 bit per
/// pixel, rows packed MSB first, 1 = black. A portrait image of the panel's
/// dimensions is rotated to landscape, as the Python driver did.
pub fn frame_from_encoded_image(encoded: &[u8], dither: Dither) -> anyhow::Result<Vec<u8>> {
    let image = image::load_from_memory(encoded).context("Failed to decode display image")?;
    frame_from_image(image, dither)
}

fn frame_from_image(image: DynamicImage, dither: Dither) -> anyhow::Result<Vec<u8>> {
    let image = match (image.width(), image.height()) {
        (WIDTH, HEIGHT) => image,
        (HEIGHT, WIDTH) => image.rotate270(),
        (w, h) => bail!("Image is {w}x{h}, but the panel is {WIDTH}x{HEIGHT}"),
    };

    let luma = quantise_grey(&image.to_luma8(), 2, dither);
    let mut frame = vec![0u8; BUFFER_SIZE];
    for (x, y, pixel) in luma.enumerate_pixels() {
        if pixel[0] < 128 {
            let index = y as usize * (WIDTH as usize / 8) + x as usize / 8;
            frame[index] |= 0x80 >> (x % 8);
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use image::{GrayImage, Luma};

    use super::*;

    #[test]
    fn packs_black_pixels_as_set_bits_msb_first() {
        let mut img = GrayImage::from_pixel(WIDTH, HEIGHT, Luma([255]));
        img.put_pixel(0, 0, Luma([0]));
        img.put_pixel(9, 1, Luma([0]));

        let frame = frame_from_image(DynamicImage::ImageLuma8(img), Dither::None).unwrap();

        assert_eq!(frame.len(), BUFFER_SIZE);
        assert_eq!(frame[0], 0b1000_0000);
        assert_eq!(frame[100 + 1], 0b0100_0000);
        assert_eq!(frame.iter().map(|b| b.count_ones()).sum::<u32>(), 2);
    }

    #[test]
    fn rotates_portrait_images_counter_clockwise() {
        // Top-left of a portrait image ends up bottom-left after a CCW turn.
        let mut img = GrayImage::from_pixel(HEIGHT, WIDTH, Luma([255]));
        img.put_pixel(0, 0, Luma([0]));

        let frame = frame_from_image(DynamicImage::ImageLuma8(img), Dither::None).unwrap();

        assert_eq!(frame[(HEIGHT as usize - 1) * 100], 0b1000_0000);
    }

    #[test]
    fn rejects_wrong_dimensions() {
        let img = GrayImage::new(100, 100);
        assert!(frame_from_image(DynamicImage::ImageLuma8(img), Dither::None).is_err());
    }
}

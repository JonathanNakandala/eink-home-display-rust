use anyhow::{Context, Error, Result};
use image::{DynamicImage, GenericImageView, ImageBuffer, Luma};

fn convert_to_1bit(img: &DynamicImage) -> Vec<u8> {
    let (width, height) = img.dimensions();
    let mut one_bit_array = Vec::new();

    for y in 0..height {
        let mut byte = 0u8;
        let mut bit_count = 0;

        for x in 0..width {
            let pixel = img.get_pixel(x, y);
            let brightness =
                0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32;
            let bit = if brightness > 128.0 { 0 } else { 1 };

            byte |= bit << (7 - bit_count);
            bit_count += 1;

            if bit_count == 8 {
                one_bit_array.push(byte);
                byte = 0;
                bit_count = 0;
            }
        }

        if bit_count > 0 {
            one_bit_array.push(byte);
        }
    }

    one_bit_array
}

pub(super) fn save_1bit_array_as_png(
    one_bit_array: &[u8],
    width: u32,
    height: u32,
    output_path: &str,
) -> Result<(), Error> {
    let mut img = ImageBuffer::new(width, height);

    for (i, &byte) in one_bit_array.iter().enumerate() {
        for bit in 0..8 {
            let x = (i * 8 + bit) as u32 % width;
            let y = (i * 8 + bit) as u32 / width;
            if y >= height {
                break;
            }
            let color = if (byte & (1 << (7 - bit))) != 0 {
                Luma([0u8])
            } else {
                Luma([255u8])
            };
            img.put_pixel(x, y, color);
        }
    }

    img.save(output_path)?;
    Ok(())
}

pub(super) fn generate_1bit_image(screenshot_data: Vec<u8>) -> Result<Vec<u8>> {
    let img =
        image::load_from_memory(&screenshot_data).context("Failed to load image from memory")?;
    let one_bit_array = convert_to_1bit(&img);
    Ok(one_bit_array)
}

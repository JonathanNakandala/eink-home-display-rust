//! Reduce a greyscale image to a few evenly spaced levels, shared by the
//! display adapters that can't show full colour.

use image::{GrayImage, Luma};

use crate::domain::models::display::Dither;

const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];

/// Snap every pixel to one of `levels` (>= 2) evenly spaced greys between 0 and 255.
pub fn quantise_grey(image: &GrayImage, levels: u8, dither: Dither) -> GrayImage {
    assert!(levels >= 2, "need at least two levels");
    let step = 255.0 / (levels - 1) as f32;
    let snap = |value: f32| (value / step).round().clamp(0.0, (levels - 1) as f32) * step;

    let (width, height) = image.dimensions();
    let mut out = GrayImage::new(width, height);

    match dither {
        Dither::None => {
            for (x, y, pixel) in image.enumerate_pixels() {
                out.put_pixel(x, y, Luma([snap(pixel[0] as f32) as u8]));
            }
        }
        Dither::Ordered => {
            for (x, y, pixel) in image.enumerate_pixels() {
                let bias = (BAYER_4X4[(y % 4) as usize][(x % 4) as usize] / 16.0 - 0.5) * step;
                out.put_pixel(x, y, Luma([snap(pixel[0] as f32 + bias) as u8]));
            }
        }
        Dither::FloydSteinberg => {
            let (w, h) = (width as usize, height as usize);
            let mut work: Vec<f32> = image.pixels().map(|p| p[0] as f32).collect();
            for y in 0..h {
                for x in 0..w {
                    let old = work[y * w + x];
                    let new = snap(old);
                    out.put_pixel(x as u32, y as u32, Luma([new as u8]));
                    let error = old - new;
                    let mut spread = |dx: isize, dy: usize, weight: f32| {
                        let nx = x as isize + dx;
                        if nx >= 0 && (nx as usize) < w && y + dy < h {
                            work[(y + dy) * w + nx as usize] += error * weight;
                        }
                    };
                    spread(1, 0, 7.0 / 16.0);
                    spread(-1, 1, 3.0 / 16.0);
                    spread(0, 1, 5.0 / 16.0);
                    spread(1, 1, 1.0 / 16.0);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_splits_at_the_midpoint() {
        let img = GrayImage::from_vec(3, 1, vec![10, 127, 200]).unwrap();
        let out = quantise_grey(&img, 2, Dither::None);
        assert_eq!(out.into_raw(), vec![0, 0, 255]);
    }

    #[test]
    fn sixteen_levels_are_evenly_spaced() {
        let img = GrayImage::from_vec(2, 1, vec![17, 250]).unwrap();
        let out = quantise_grey(&img, 16, Dither::None);
        assert_eq!(out.into_raw(), vec![17, 255]);
    }

    #[test]
    fn dithering_keeps_mid_grey_roughly_half_black() {
        let img = GrayImage::from_pixel(32, 32, Luma([128]));
        for dither in [Dither::FloydSteinberg, Dither::Ordered] {
            let out = quantise_grey(&img, 2, dither);
            let white = out.pixels().filter(|p| p[0] == 255).count() as f32;
            let ratio = white / (32.0 * 32.0);
            assert!((0.4..0.6).contains(&ratio), "{dither:?} gave {ratio}");
        }
    }

    #[test]
    fn output_only_contains_allowed_levels() {
        let img = GrayImage::from_fn(16, 16, |x, y| Luma([(x * 16 + y) as u8]));
        let out = quantise_grey(&img, 4, Dither::FloydSteinberg);
        assert!(out.pixels().all(|p| [0, 85, 170, 255].contains(&p[0])));
    }
}

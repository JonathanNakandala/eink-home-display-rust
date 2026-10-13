//! The icon of the page, for a browser to put on a home screen: a picture frame, drawn here so that there are no files to
//! carry, as a vector (for the browsers that take one) and as the PNGs the others want (a manifest's 192 and 512, and the
//! 180 an Apple device looks for).
//!
//! It is a dark rounded square with a light screen in it and two lines on the screen, in proportions that stay clear when it
//! is small and survive being cropped to a circle, as a phone does.

use std::io::Cursor;

use image::{ImageFormat, Rgba, RgbaImage};

/// The sizes served, in pixels.
pub const SIZES: [u32; 3] = [180, 192, 512];

const DARK: [u8; 4] = [17, 17, 17, 255];
const LIGHT: [u8; 4] = [255, 255, 255, 255];
const GREY: [u8; 4] = [150, 150, 150, 255];

/// The icon as an SVG, drawn on a 512 by 512 grid.
pub const SVG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 512 512\">\
<rect width=\"512\" height=\"512\" rx=\"96\" fill=\"#111\"/>\
<rect x=\"96\" y=\"128\" width=\"320\" height=\"256\" rx=\"24\" fill=\"#fff\"/>\
<rect x=\"140\" y=\"176\" width=\"232\" height=\"28\" rx=\"8\" fill=\"#111\"/>\
<rect x=\"140\" y=\"232\" width=\"160\" height=\"20\" rx=\"8\" fill=\"#969696\"/>\
<rect x=\"140\" y=\"276\" width=\"200\" height=\"20\" rx=\"8\" fill=\"#969696\"/>\
</svg>";

/// Whether `(x, y)` is inside a rectangle with rounded corners of radius `radius`.
fn inside(x: f32, y: f32, left: f32, top: f32, width: f32, height: f32, radius: f32) -> bool {
    let (right, bottom) = (left + width, top + height);
    if x < left || x >= right || y < top || y >= bottom {
        return false;
    }
    // Within a corner's square, inside its circle.
    let cx = if x < left + radius {
        left + radius
    } else if x > right - radius {
        right - radius
    } else {
        return true;
    };
    let cy = if y < top + radius {
        top + radius
    } else if y > bottom - radius {
        bottom - radius
    } else {
        return true;
    };
    (x - cx).powi(2) + (y - cy).powi(2) <= radius.powi(2)
}

/// The icon as a PNG, `size` pixels square, drawn from the same shapes as `SVG` (which are on a grid of 512).
pub fn png(size: u32) -> Vec<u8> {
    let scale = 512.0 / size as f32;
    let image = RgbaImage::from_fn(size, size, |px, py| {
        // The middle of the pixel, on the grid of the SVG.
        let (x, y) = ((px as f32 + 0.5) * scale, (py as f32 + 0.5) * scale);
        if !inside(x, y, 0.0, 0.0, 512.0, 512.0, 96.0) {
            return Rgba([0, 0, 0, 0]);
        }
        let colour = if inside(x, y, 140.0, 176.0, 232.0, 28.0, 8.0) {
            DARK
        } else if inside(x, y, 140.0, 232.0, 160.0, 20.0, 8.0)
            || inside(x, y, 140.0, 276.0, 200.0, 20.0, 8.0)
        {
            GREY
        } else if inside(x, y, 96.0, 128.0, 320.0, 256.0, 24.0) {
            LIGHT
        } else {
            DARK
        };
        Rgba(colour)
    });
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .expect("an image encodes to memory");
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_size_is_a_png_of_that_size_with_the_frame_in_it() {
        for size in SIZES {
            let image = image::load_from_memory(&png(size)).unwrap().to_rgba8();

            assert_eq!(image.dimensions(), (size, size));
            // The corner is clear (rounded away), the middle of the screen is light, the title line is dark.
            assert_eq!(image.get_pixel(0, 0).0[3], 0, "{size}");
            let at = |x: f32, y: f32| {
                image
                    .get_pixel(
                        (x / 512.0 * size as f32) as u32,
                        (y / 512.0 * size as f32) as u32,
                    )
                    .0
            };
            assert_eq!(at(256.0, 360.0), LIGHT, "{size}");
            assert_eq!(at(256.0, 190.0), DARK, "{size}");
            assert_eq!(at(200.0, 242.0), GREY, "{size}");
            // Outside the screen but inside the icon is dark.
            assert_eq!(at(40.0, 256.0), DARK, "{size}");
        }
    }

    #[test]
    fn the_svg_is_one_document_that_draws_the_same_shapes() {
        assert!(SVG.starts_with("<svg ") && SVG.ends_with("</svg>"));
        assert_eq!(SVG.matches("<svg").count(), 1);
        // Every shape the PNG draws is in it, by the same numbers.
        for shape in [
            "rx=\"96\"",
            "x=\"96\" y=\"128\" width=\"320\" height=\"256\"",
            "x=\"140\" y=\"176\" width=\"232\" height=\"28\"",
        ] {
            assert!(SVG.contains(shape), "{shape}");
        }
        // Nothing that runs, nothing fetched.
        assert!(!SVG.contains("<script") && !SVG.contains("href"));
    }
}

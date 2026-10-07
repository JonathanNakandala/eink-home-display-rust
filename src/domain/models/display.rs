/// What a display can show. The image generator renders to this, and the
/// display adapter converts the result to its own pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayProfile {
    /// Size in pixels of the image to render, in the orientation the layout is designed for.
    pub width: u32,
    pub height: u32,
    pub palette: Palette,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// Black and white.
    Mono,
    /// Evenly spaced grey levels, e.g. 4 or 16.
    Grey(u8),
    /// A fixed set of RGB inks, e.g. a 7-colour panel.
    Indexed(&'static [[u8; 3]]),
    /// Full colour.
    Rgb,
}

/// How to reduce the rendered image to fewer levels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Dither {
    /// Plain threshold: crisp text, harsh on greys.
    #[default]
    None,
    /// Error diffusion: smooth greys, a bit noisy on text.
    FloydSteinberg,
    /// 4x4 Bayer matrix: regular pattern, stable between refreshes.
    Ordered,
}

/// The file format the reTerminal E1003 downloads.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// 8-bit greyscale, uncompressed (about 2.6 MB). The simplest for the firmware to decode.
    #[default]
    Bmp,
    /// 8-bit greyscale, compressed (well under 200 KB). Less to download, but the firmware has to inflate it.
    Png,
    /// Lossless and about as small as PNG here (a little larger), but decoded in one cheap pass with no inflate
    /// step. QOI has no greyscale form, so the greys are stored as RGB.
    Qoi,
}

impl ImageFormat {
    /// Every format the server can produce, for announcing what it is able to serve.
    pub const ALL: [ImageFormat; 3] = [Self::Bmp, Self::Png, Self::Qoi];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Bmp => "bmp",
            Self::Png => "png",
            Self::Qoi => "qoi",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Bmp => "image/bmp",
            Self::Png => "image/png",
            Self::Qoi => "image/qoi",
        }
    }
}

//! What a display says it is when it asks to join: its model, its panel, the image formats it can decode, and the firmware it
//! runs. The owner sees it before approving, to know which display is asking, and the server can tell a display it does not
//! expect from one it does.
//!
//! It is self-asserted: the display wrote it, and the pairing code does not cover it. So it is text for the owner, bounded and
//! restricted to plain characters, shown escaped; it is never used to decide whether a display may join.

use std::fmt;

use thiserror::Error;

/// The longest firmware version or model, in characters.
pub const FIRMWARE_CHARS: usize = 32;
pub(crate) const MODEL_CHARS: usize = 32;
pub(crate) const FORMAT_CHARS: usize = 16;
pub(crate) const MAX_FORMATS: usize = 8;
/// Panels are a few thousand pixels across; anything past this is a misread.
pub(crate) const MAX_PIXELS: u32 = 20_000;
/// Grey levels: two (black and white) to a byte's worth.
pub(crate) const LEVELS: std::ops::RangeInclusive<u32> = 2..=256;

/// A firmware version as the display writes it: letters, digits, dots, hyphens and underscores, at most `FIRMWARE_CHARS`. A
/// release is `0.2.0` or `0.3.0-beta.1`.
pub fn firmware_version(text: &str) -> Option<String> {
    let usable = !text.is_empty()
        && text.len() <= FIRMWARE_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    usable.then(|| text.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProfile {
    /// What the display is: `reTerminal E1003`.
    pub model: String,
    /// The release of its firmware.
    pub firmware: String,
    /// The panel in pixels.
    pub width: u32,
    pub height: u32,
    /// How many greys it shows.
    pub levels: u32,
    /// The image formats it can decode, as short names (`bmp`, `png`, `qoi`).
    pub formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidProfile {
    #[error("the model is empty, too long or has characters a name does not")]
    Model,
    #[error("the firmware version is empty, too long or has characters a version does not")]
    Firmware,
    #[error("the panel is {0} by {1} pixels")]
    Panel(u32, u32),
    #[error("{0} grey levels")]
    Levels(u32),
    #[error("an image format is empty, too long or has characters a name does not")]
    Format,
    #[error("more than {MAX_FORMATS} image formats")]
    TooManyFormats,
}

impl DeviceProfile {
    /// A profile, if everything in it is usable. Nothing is repaired: a profile with something wrong in it is not kept.
    pub fn new(
        model: &str,
        firmware: &str,
        width: u32,
        height: u32,
        levels: u32,
        formats: Vec<String>,
    ) -> Result<Self, InvalidProfile> {
        let model_usable = !model.is_empty()
            && model.chars().count() <= MODEL_CHARS
            && model.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | ':' | '+')
            });
        if !model_usable {
            return Err(InvalidProfile::Model);
        }
        let firmware = firmware_version(firmware).ok_or(InvalidProfile::Firmware)?;
        if !(1..=MAX_PIXELS).contains(&width) || !(1..=MAX_PIXELS).contains(&height) {
            return Err(InvalidProfile::Panel(width, height));
        }
        if !LEVELS.contains(&levels) {
            return Err(InvalidProfile::Levels(levels));
        }
        if formats.len() > MAX_FORMATS {
            return Err(InvalidProfile::TooManyFormats);
        }
        for format in &formats {
            let usable = !format.is_empty()
                && format.len() <= FORMAT_CHARS
                && format.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-')
                });
            if !usable {
                return Err(InvalidProfile::Format);
            }
        }
        Ok(Self {
            model: model.to_owned(),
            firmware,
            width,
            height,
            levels,
            formats,
        })
    }
}

/// `reTerminal E1003, 1872x1404, 16 greys, firmware 0.2.0`: safe in a log line, as every part is plain characters.
impl fmt::Display for DeviceProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}, {}x{}, {} greys, firmware {}",
            self.model, self.width, self.height, self.levels, self.firmware
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn good() -> DeviceProfile {
        DeviceProfile::new(
            "reTerminal E1003",
            "0.2.0",
            1872,
            1404,
            16,
            formats(&["bmp", "png", "qoi"]),
        )
        .unwrap()
    }

    #[test]
    fn a_usable_profile_is_kept_whole_and_said_in_a_line() {
        let profile = good();
        assert_eq!(profile.model, "reTerminal E1003");
        assert_eq!(
            (profile.width, profile.height, profile.levels),
            (1872, 1404, 16)
        );
        assert_eq!(
            profile.to_string(),
            "reTerminal E1003, 1872x1404, 16 greys, firmware 0.2.0"
        );
    }

    #[test]
    fn something_wrong_anywhere_means_no_profile_and_nothing_is_repaired() {
        let build = |model: &str, firmware: &str, w, h, levels, f: &[&str]| {
            DeviceProfile::new(model, firmware, w, h, levels, formats(f))
        };
        assert_eq!(
            build("", "0.2.0", 1, 1, 16, &[]),
            Err(InvalidProfile::Model)
        );
        assert_eq!(
            build(&"m".repeat(33), "0.2.0", 1, 1, 16, &[]),
            Err(InvalidProfile::Model)
        );
        assert_eq!(
            build("<script>", "0.2.0", 1, 1, 16, &[]),
            Err(InvalidProfile::Model)
        );
        assert_eq!(
            build("m\nx", "0.2.0", 1, 1, 16, &[]),
            Err(InvalidProfile::Model)
        );
        assert_eq!(build("m", "", 1, 1, 16, &[]), Err(InvalidProfile::Firmware));
        assert_eq!(
            build("m", "1.0.0+x", 1, 1, 16, &[]),
            Err(InvalidProfile::Firmware)
        );
        assert_eq!(
            build("m", "0.2.0", 0, 1, 16, &[]),
            Err(InvalidProfile::Panel(0, 1))
        );
        assert_eq!(
            build("m", "0.2.0", 1, 20_001, 16, &[]),
            Err(InvalidProfile::Panel(1, 20_001))
        );
        assert_eq!(
            build("m", "0.2.0", 1, 1, 1, &[]),
            Err(InvalidProfile::Levels(1))
        );
        assert_eq!(
            build("m", "0.2.0", 1, 1, 257, &[]),
            Err(InvalidProfile::Levels(257))
        );
        assert_eq!(
            build("m", "0.2.0", 1, 1, 16, &["BMP"]),
            Err(InvalidProfile::Format)
        );
        assert_eq!(
            build("m", "0.2.0", 1, 1, 16, &[""]),
            Err(InvalidProfile::Format)
        );
        assert_eq!(
            build(
                "m",
                "0.2.0",
                1,
                1,
                16,
                &["a", "b", "c", "d", "e", "f", "g", "h", "i"]
            ),
            Err(InvalidProfile::TooManyFormats)
        );
    }

    #[test]
    fn the_limits_themselves_are_allowed() {
        assert!(
            DeviceProfile::new(
                &"m".repeat(32),
                &"1".repeat(32),
                20_000,
                20_000,
                256,
                formats(&["a"; 8])
            )
            .is_ok()
        );
        assert!(DeviceProfile::new("m", "0", 1, 1, 2, vec![]).is_ok());
    }

    #[test]
    fn a_firmware_version_is_a_release_or_a_prerelease() {
        for good in ["0.2.0", "0.3.0-beta.1", "dev_build", "1"] {
            assert_eq!(firmware_version(good).as_deref(), Some(good));
        }
        for bad in ["", "1.0.0+build", "a b", "x\n", &"1".repeat(33)] {
            assert_eq!(firmware_version(bad), None, "{bad:?}");
        }
    }
}

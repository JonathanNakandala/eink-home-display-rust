//! Picks which format to send for a request's `Accept` header (RFC 9110 section 12): the client
//! lists the media types it can decode, each optionally weighted with `q=`, and the server chooses
//! among what it has. The display's decoder is chosen by the reply's `Content-Type`, so whichever
//! is picked works.
//!
//! The rules followed: the most specific range that matches a type decides its weight (`image/bmp`
//! over `image/*` over `*/*`), `q=0` means "not acceptable", the highest weight wins, and a tie goes
//! to the server's preferred format. No `Accept` header means the client takes anything.

use crate::domain::models::display::ImageFormat;

struct Range {
    /// How specific the range is: 2 for `type/subtype`, 1 for `type/*`, 0 for `*/*`.
    specificity: u8,
    media: String,
    weight: u16,
}

/// The formats from `available`, in the order they would be chosen for `accept`, best first and
/// without any the client refuses. `preferred` breaks ties.
pub fn acceptable(
    accept: Option<&str>,
    preferred: ImageFormat,
    available: &[ImageFormat],
) -> Vec<ImageFormat> {
    let mut candidates: Vec<(u16, usize, ImageFormat)> = available
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(order, format)| {
            let weight = weight_of(accept, format)?;
            // Higher weight first, then the preferred format, then the order given.
            Some((
                weight,
                usize::from(format != preferred) * 1000 + order,
                format,
            ))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    candidates
        .into_iter()
        .map(|(_, _, format)| format)
        .collect()
}

/// The weight (0 to 1000) the client gives `format`, or None if it is refused (not listed, or q=0).
fn weight_of(accept: Option<&str>, format: ImageFormat) -> Option<u16> {
    let Some(accept) = accept.map(str::trim).filter(|accept| !accept.is_empty()) else {
        return Some(1000);
    };
    let media = format.content_type();
    let best = accept
        .split(',')
        .filter_map(parse_range)
        .filter(|range| matches(&range.media, media))
        .max_by_key(|range| range.specificity)?;
    (best.weight > 0).then_some(best.weight)
}

fn matches(range: &str, media: &str) -> bool {
    match range {
        "*/*" => true,
        _ => match range.strip_suffix("/*") {
            Some(kind) => media.split('/').next() == Some(kind),
            None => range == media,
        },
    }
}

/// One `type/subtype;q=0.8` item; None if it isn't a media range, so a malformed one is skipped.
fn parse_range(item: &str) -> Option<Range> {
    let mut parts = item.split(';');
    let media = parts.next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = media.split_once('/')?;
    if kind.is_empty() || subtype.is_empty() || (kind == "*" && subtype != "*") {
        return None;
    }
    let mut weight = 1000;
    for parameter in parts {
        if let Some((name, value)) = parameter.split_once('=') {
            if name.trim().eq_ignore_ascii_case("q") {
                // A malformed weight makes the range unusable rather than guessing what was meant.
                let q: f32 = value
                    .trim()
                    .parse()
                    .ok()
                    .filter(|q| (0.0..=1.0).contains(q))?;
                weight = (q * 1000.0).round() as u16;
            }
        }
    }
    let specificity = match (kind, subtype) {
        ("*", _) => 0,
        (_, "*") => 1,
        _ => 2,
    };
    Some(Range {
        specificity,
        media,
        weight,
    })
}

#[cfg(test)]
mod tests {
    use ImageFormat::{Bmp, Png, Qoi};

    use super::*;

    const BOTH: [ImageFormat; 2] = [Bmp, Png];

    fn pick(accept: Option<&str>, preferred: ImageFormat) -> Vec<ImageFormat> {
        acceptable(accept, preferred, &BOTH)
    }

    #[test]
    fn no_accept_header_gives_the_preferred_format_first() {
        assert_eq!(pick(None, Bmp), [Bmp, Png]);
        assert_eq!(pick(None, Png), [Png, Bmp]);
        assert_eq!(pick(Some(""), Png), [Png, Bmp]);
        assert_eq!(pick(Some("  "), Png), [Png, Bmp]);
    }

    #[test]
    fn a_specific_type_is_all_that_is_sent() {
        assert_eq!(pick(Some("image/png"), Bmp), [Png]);
        assert_eq!(pick(Some("image/bmp"), Png), [Bmp]);
    }

    #[test]
    fn wildcards_accept_everything_and_leave_the_choice_to_the_server() {
        assert_eq!(pick(Some("*/*"), Png), [Png, Bmp]);
        assert_eq!(pick(Some("image/*"), Bmp), [Bmp, Png]);
        // What ESPHome sends for format AUTO.
        assert_eq!(pick(Some("image/*,*/*;q=0.8"), Png), [Png, Bmp]);
    }

    #[test]
    fn weights_decide_between_formats_the_client_accepts() {
        assert_eq!(pick(Some("image/bmp;q=0.5, image/png"), Bmp), [Png, Bmp]);
        assert_eq!(
            pick(Some("image/png;q=0.2, image/bmp;q=0.9"), Png),
            [Bmp, Png]
        );
    }

    #[test]
    fn equal_weights_go_to_the_servers_preference() {
        assert_eq!(pick(Some("image/bmp, image/png"), Png), [Png, Bmp]);
        assert_eq!(pick(Some("image/bmp, image/png"), Bmp), [Bmp, Png]);
    }

    #[test]
    fn the_most_specific_range_decides_not_the_first_or_the_widest() {
        // bmp is named, so its own weight applies, not the wildcard's.
        assert_eq!(
            pick(Some("image/*;q=0.3, image/bmp;q=0.9"), Png),
            [Bmp, Png]
        );
        assert_eq!(
            pick(Some("*/*;q=0.1, image/*;q=0.5, image/png"), Bmp),
            [Png, Bmp]
        );
        // Naming a type with q=0 refuses it even though a wildcard would allow it.
        assert_eq!(pick(Some("*/*, image/png;q=0"), Png), [Bmp]);
    }

    #[test]
    fn qoi_is_offered_like_any_other_format() {
        let all = ImageFormat::ALL;
        assert_eq!(acceptable(Some("image/qoi"), Bmp, &all), [Qoi]);
        assert_eq!(
            acceptable(Some("image/qoi, image/png;q=0.5"), Bmp, &all),
            [Qoi, Png]
        );
        // Equal weights: the server's preference, then the order of ALL.
        assert_eq!(
            acceptable(Some("image/bmp, image/png, image/qoi"), Qoi, &all),
            [Qoi, Bmp, Png]
        );
        assert_eq!(acceptable(Some("image/*"), Bmp, &all), [Bmp, Png, Qoi]);
        // A device that can't decode it never gets it.
        assert_eq!(
            acceptable(Some("image/bmp, image/png"), Qoi, &all),
            [Bmp, Png]
        );
        assert_eq!(
            acceptable(Some("*/*, image/qoi;q=0"), Qoi, &all),
            [Bmp, Png]
        );
    }

    #[test]
    fn nothing_acceptable_gives_an_empty_list() {
        assert!(pick(Some("image/gif"), Bmp).is_empty());
        assert!(pick(Some("text/html, application/json"), Bmp).is_empty());
        assert!(pick(Some("image/png;q=0, image/bmp;q=0"), Bmp).is_empty());
    }

    #[test]
    fn only_available_formats_are_offered() {
        assert_eq!(acceptable(Some("*/*"), Png, &[Bmp]), [Bmp]);
        assert!(acceptable(Some("image/png"), Png, &[Bmp]).is_empty());
        assert!(acceptable(None, Bmp, &[]).is_empty());
    }

    #[test]
    fn is_case_insensitive_and_tolerant_of_spacing() {
        assert_eq!(
            pick(Some("IMAGE/PNG ; Q=0.5 ,  Image/Bmp"), Png),
            [Bmp, Png]
        );
    }

    #[test]
    fn malformed_ranges_are_skipped_not_fatal() {
        assert_eq!(pick(Some("garbage, image/png"), Bmp), [Png]);
        assert_eq!(pick(Some("image/png;q=lots, image/bmp"), Png), [Bmp]);
        assert_eq!(pick(Some("image/png;q=1.5, image/bmp"), Png), [Bmp]);
        assert_eq!(
            pick(Some("*/png, /bmp, image/"), Png),
            Vec::<ImageFormat>::new()
        );
    }
}

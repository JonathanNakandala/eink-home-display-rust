//! When the first render happens after the process starts.
//!
//! A service that crashes and is restarted at once would otherwise render, and so call the
//! weather and departure APIs, on every start. Two things stop that: the first render is
//! skipped when the served image is still current, and it waits out a cooldown since the last
//! render was attempted. The attempt is recorded in a file, so the cooldown holds across
//! restarts. Failed renders never end the process, so only a crash can restart it.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local};

use crate::adapters::image_server::{rendered_at, render_due};
use crate::domain::models::display::ImageFormat;
use crate::scheduler::Schedule;

/// When to make the first render, or None to wait for the first scheduled slot.
pub fn first_render_at(
    now: DateTime<Local>,
    skip: bool,
    image_is_current: bool,
    last_attempt: Option<DateTime<Local>>,
    cooldown: Duration,
) -> Option<DateTime<Local>> {
    if skip || image_is_current {
        return None;
    }
    let earliest = last_attempt.and_then(|at| chrono::Duration::from_std(cooldown).ok().map(|c| at + c));
    Some(earliest.map_or(now, |earliest| earliest.max(now)))
}

/// Whether the image being served has no render due yet, so it needn't be redone at start-up.
pub fn served_image_is_current(
    directory: &Path,
    format: ImageFormat,
    schedule: &Schedule,
    now: DateTime<Local>,
) -> bool {
    rendered_at(directory, format)
        .and_then(|rendered| render_due(now, rendered, schedule).ok())
        .is_some_and(|due| !due)
}

/// Notes that a render is starting. Best effort: a render is worth more than its bookkeeping.
pub fn record_attempt(marker: &Path) {
    if let Err(e) = std::fs::write(marker, Local::now().to_rfc3339()) {
        log::warn!("Could not record the render attempt in {}: {e}", marker.display());
    }
}

pub fn last_attempt(marker: &Path) -> Option<DateTime<Local>> {
    std::fs::metadata(marker).ok()?.modified().ok().map(DateTime::<Local>::from)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const COOLDOWN: Duration = Duration::from_secs(120);

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    #[test]
    fn renders_at_once_on_a_first_start() {
        assert_eq!(first_render_at(at(12, 0, 0), false, false, None, COOLDOWN), Some(at(12, 0, 0)));
    }

    #[test]
    fn waits_out_the_cooldown_after_a_recent_attempt() {
        let recent = Some(at(11, 59, 30));
        assert_eq!(first_render_at(at(12, 0, 0), false, false, recent, COOLDOWN), Some(at(12, 1, 30)));
    }

    #[test]
    fn an_old_attempt_does_not_delay_anything() {
        let old = Some(at(11, 0, 0));
        assert_eq!(first_render_at(at(12, 0, 0), false, false, old, COOLDOWN), Some(at(12, 0, 0)));
    }

    #[test]
    fn a_current_image_or_the_skip_flag_waits_for_the_first_slot() {
        assert_eq!(first_render_at(at(12, 0, 0), false, true, None, COOLDOWN), None);
        assert_eq!(first_render_at(at(12, 0, 0), true, false, None, COOLDOWN), None);
    }

    #[test]
    fn the_attempt_marker_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("last_render_attempt");
        assert!(last_attempt(&marker).is_none());

        record_attempt(&marker);
        let seen = last_attempt(&marker).unwrap();
        assert!((Local::now() - seen).num_seconds().abs() < 5);
    }

    #[test]
    fn an_image_is_current_until_its_next_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let schedule = Schedule::parse_every("1h").unwrap();
        let now = Local::now();
        assert!(!served_image_is_current(tmp.path(), ImageFormat::Bmp, &schedule, now));

        std::fs::write(tmp.path().join("image.bmp"), b"x").unwrap();
        assert!(served_image_is_current(tmp.path(), ImageFormat::Bmp, &schedule, now));
        assert!(!served_image_is_current(tmp.path(), ImageFormat::Bmp, &schedule, now + chrono::Duration::hours(2)));
    }
}

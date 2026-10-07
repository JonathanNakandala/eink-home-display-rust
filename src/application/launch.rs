//! When the first render happens after the process starts.
//!
//! A service that crashes and is restarted at once would otherwise render, and so call the
//! weather and departure APIs, on every start. Two things stop that: the first render is
//! skipped when the served image is still current, and it waits out a cooldown since the last
//! render was attempted. The attempt is recorded in a file, so the cooldown holds across
//! restarts. Failed renders never end the process, so only a crash can restart it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::application::plan::render_due;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::render_report::RenderReport;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::published_images::PublishedImages;
use crate::domain::services::render_observer::RenderObserver;

/// When to make the first render, or None to wait for the first scheduled slot.
pub fn first_render_at(
    now: DateTime<Tz>,
    skip: bool,
    image_is_current: bool,
    last_attempt: Option<DateTime<Utc>>,
    cooldown: Duration,
) -> Option<DateTime<Tz>> {
    if skip || image_is_current {
        return None;
    }
    // An attempt later than now can't be real: the clock was ahead when it was written (or has been
    // corrected since). Counted from there, the cooldown would hold off every render until then, so it
    // is counted from now instead.
    let earliest = last_attempt.and_then(|at| {
        chrono::Duration::from_std(cooldown).ok().and_then(|c| {
            at.with_timezone(&now.timezone())
                .min(now)
                .checked_add_signed(c)
        })
    });
    Some(earliest.map_or(now, |earliest| earliest.max(now)))
}

/// Whether the image being served has no render due yet, so it needn't be redone at start-up.
pub async fn served_image_is_current(
    images: &dyn PublishedImages,
    format: ImageFormat,
    schedule: &Schedule,
    now: DateTime<Tz>,
) -> bool {
    images
        .published_at(format)
        .await
        .ok()
        .flatten()
        .and_then(|rendered| {
            render_due(now, rendered.with_timezone(&now.timezone()), schedule).ok()
        })
        .is_some_and(|due| !due)
}

/// Records each render's start in a file (the time, from the clock), for `first_render_at`'s
/// cooldown after a restart. Best effort: a render is worth more than its bookkeeping.
pub struct AttemptMarker {
    path: PathBuf,
    clock: Arc<dyn Clock>,
}

impl AttemptMarker {
    pub fn new(path: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { path, clock }
    }
}

impl RenderObserver for AttemptMarker {
    fn render_started(&self) {
        if let Err(e) = std::fs::write(&self.path, self.clock.now().to_rfc3339()) {
            log::warn!(
                "Could not record the render attempt in {}: {e}",
                self.path.display()
            );
        }
    }

    fn render_succeeded(&self, _at: DateTime<Tz>, _report: &RenderReport) {}

    fn render_failed(&self, _at: DateTime<Tz>, _error: &anyhow::Error) {}
}

/// When the last render was started, from the marker; None if there is none or it can't be read.
pub fn last_attempt(marker: &Path) -> Option<DateTime<Utc>> {
    let text = std::fs::read_to_string(marker).ok()?;
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    use super::*;
    use crate::adapters::published_images::DirectoryImages;

    const COOLDOWN: Duration = Duration::from_secs(120);

    fn at(h: u32, m: u32, s: u32) -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    /// The same moment as an instant, which is how the marker and the published image are read.
    fn utc(h: u32, m: u32, s: u32) -> DateTime<Utc> {
        at(h, m, s).with_timezone(&Utc)
    }

    #[test]
    fn renders_at_once_on_a_first_start() {
        assert_eq!(
            first_render_at(at(12, 0, 0), false, false, None, COOLDOWN),
            Some(at(12, 0, 0))
        );
    }

    #[test]
    fn waits_out_the_cooldown_after_a_recent_attempt() {
        let recent = Some(utc(11, 59, 30));
        assert_eq!(
            first_render_at(at(12, 0, 0), false, false, recent, COOLDOWN),
            Some(at(12, 1, 30))
        );
    }

    #[test]
    fn an_attempt_in_the_future_counts_from_now_not_from_then() {
        // Written while the clock was a day ahead: the render must not wait a day for it.
        let future = Some(utc(12, 0, 0) + chrono::Duration::days(1));
        assert_eq!(
            first_render_at(at(12, 0, 0), false, false, future, COOLDOWN),
            Some(at(12, 2, 0))
        );
    }

    #[test]
    fn a_cooldown_too_large_to_add_is_ignored_not_a_panic() {
        let huge = Duration::from_secs(u64::MAX / 2);
        assert_eq!(
            first_render_at(at(12, 0, 0), false, false, Some(utc(11, 59, 0)), huge),
            Some(at(12, 0, 0))
        );
    }

    #[test]
    fn an_old_attempt_does_not_delay_anything() {
        let old = Some(utc(11, 0, 0));
        assert_eq!(
            first_render_at(at(12, 0, 0), false, false, old, COOLDOWN),
            Some(at(12, 0, 0))
        );
    }

    #[test]
    fn a_current_image_or_the_skip_flag_waits_for_the_first_slot() {
        assert_eq!(
            first_render_at(at(12, 0, 0), false, true, None, COOLDOWN),
            None
        );
        assert_eq!(
            first_render_at(at(12, 0, 0), true, false, None, COOLDOWN),
            None
        );
    }

    #[test]
    fn the_attempt_marker_records_the_clocks_time() {
        use crate::adapters::clock::FixedClock;

        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("last_render_attempt");
        assert!(last_attempt(&marker).is_none());

        // The clock, not the file's modification time, says when the attempt was.
        let clock = FixedClock::at(at(9, 30, 0));
        AttemptMarker::new(marker.clone(), clock.clone()).render_started();
        assert_eq!(last_attempt(&marker), Some(utc(9, 30, 0)));

        clock.set(at(9, 45, 0));
        AttemptMarker::new(marker.clone(), clock).render_started();
        assert_eq!(last_attempt(&marker), Some(utc(9, 45, 0)));
    }

    #[test]
    fn an_unreadable_marker_is_no_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("last_render_attempt");
        std::fs::write(&marker, "not a time").unwrap();
        assert!(last_attempt(&marker).is_none());
    }

    #[tokio::test]
    async fn an_image_is_current_until_its_next_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        let schedule = Schedule::parse_every("1h").unwrap();
        let now = chrono::Utc::now().with_timezone(&London);
        assert!(!served_image_is_current(&images, ImageFormat::Bmp, &schedule, now).await);

        images.publish(ImageFormat::Bmp, b"x").await.unwrap();
        assert!(served_image_is_current(&images, ImageFormat::Bmp, &schedule, now).await);
        assert!(
            !served_image_is_current(
                &images,
                ImageFormat::Bmp,
                &schedule,
                now + chrono::Duration::hours(2)
            )
            .await
        );
    }
}

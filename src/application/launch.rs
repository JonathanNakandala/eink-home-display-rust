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

use crate::application::plan::render_due;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{LayoutName, Layouts};
use crate::domain::models::render_report::RenderReport;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::published_images::PublishedImages;
use crate::domain::services::render_observer::RenderObserver;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};

/// When to make the first render, or None to wait for the first scheduled slot.
pub fn first_render_at(
    now: ZonedDateTime,
    skip: bool,
    image_is_current: bool,
    last_attempt: Option<Timestamp>,
    cooldown: Duration,
) -> Option<ZonedDateTime> {
    if skip || image_is_current {
        return None;
    }
    // An attempt later than now can't be real: the clock was ahead when it was written (or has been
    // corrected since). Counted from there, the cooldown would hold off every render until then, so it
    // is counted from now instead.
    let earliest = last_attempt.and_then(|at| {
        let cooldown = SignedDuration::try_from(cooldown).ok()?;
        at.min(now.timestamp()).checked_add(cooldown).ok()
    });
    let earliest = earliest.map(|at| at.to_zoned_datetime(now.time_zone().clone()));
    Some(match earliest {
        Some(earliest) => earliest.max(now),
        None => now,
    })
}

/// Whether the image of `layout` being served has no render due yet, so it needn't be redone at start-up.
pub async fn served_image_is_current(
    images: &dyn PublishedImages,
    layout: &LayoutName,
    format: ImageFormat,
    schedule: &Schedule,
    now: ZonedDateTime,
) -> bool {
    images
        .published_at(layout, format)
        .await
        .ok()
        .flatten()
        .and_then(|rendered| {
            render_due(
                now.clone(),
                rendered.to_zoned_datetime(now.time_zone().clone()),
                schedule,
            )
            .ok()
        })
        .is_some_and(|due| !due)
}

/// Whether every layout's image being served has no render due yet. One layout missing or out of date is enough for a
/// render at start-up: a layout added since the last run has no image at all, and would otherwise be a 404 until the
/// next scheduled render.
pub async fn served_images_are_current(
    images: &dyn PublishedImages,
    layouts: &Layouts,
    format: ImageFormat,
    schedule: &Schedule,
    now: ZonedDateTime,
) -> bool {
    for layout in layouts.all() {
        if !served_image_is_current(images, &layout.name, format, schedule, now.clone()).await {
            return false;
        }
    }
    true
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
        if let Err(e) = std::fs::write(&self.path, self.clock.instant().to_string()) {
            tracing::warn!(
                "Could not record the render attempt in {}: {e}",
                self.path.display()
            );
        }
    }

    fn render_succeeded(&self, _at: ZonedDateTime, _report: &RenderReport) {}

    fn render_failed(&self, _at: ZonedDateTime, _error: &anyhow::Error) {}
}

/// When the last render was started, from the marker; None if there is none or it can't be read.
pub fn last_attempt(marker: &Path) -> Option<Timestamp> {
    let text = std::fs::read_to_string(marker).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use crate::domain::services::clock::from_days;
    use crate::domain::services::clock::{local, london};
    use jiff::SignedDuration;

    use super::*;
    use crate::adapters::published_images::DirectoryImages;

    const COOLDOWN: Duration = Duration::from_secs(120);

    fn at(h: i8, m: i8, s: i8) -> ZonedDateTime {
        local(&london(), 2026, 6, 15, h, m, s)
    }

    /// The same moment as an instant, which is how the marker and the published image are read.
    fn utc(h: i8, m: i8, s: i8) -> Timestamp {
        at(h, m, s).timestamp()
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
        let future = Some(utc(12, 0, 0) + from_days(1));
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
        let dashboard = || LayoutName::parse("dashboard").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        let schedule = Schedule::parse_every("1h").unwrap();
        let now = Timestamp::now().to_zoned_datetime(london());
        assert!(
            !served_image_is_current(
                &images,
                &dashboard(),
                ImageFormat::Bmp,
                &schedule,
                now.clone()
            )
            .await
        );

        images
            .publish(&dashboard(), ImageFormat::Bmp, b"x")
            .await
            .unwrap();
        assert!(
            served_image_is_current(
                &images,
                &dashboard(),
                ImageFormat::Bmp,
                &schedule,
                now.clone()
            )
            .await
        );
        assert!(
            !served_image_is_current(
                &images,
                &dashboard(),
                ImageFormat::Bmp,
                &schedule,
                now.clone() + SignedDuration::from_hours(2)
            )
            .await
        );
    }

    #[tokio::test]
    async fn the_images_are_current_only_when_every_layout_has_one() {
        let tmp = tempfile::tempdir().unwrap();
        let images = DirectoryImages::new(tmp.path());
        let schedule = Schedule::parse_every("1h").unwrap();
        let now = Timestamp::now().to_zoned_datetime(london());
        let layouts = Layouts::new(vec![
            crate::domain::models::layout::Layout::new("dashboard", "First"),
            crate::domain::models::layout::Layout::new("added", "Added since the last run"),
        ])
        .unwrap();
        let dashboard = LayoutName::parse("dashboard").unwrap();
        images
            .publish(&dashboard, ImageFormat::Bmp, b"x")
            .await
            .unwrap();

        // The first layout is current, but the one added since has never been drawn.
        assert!(
            served_image_is_current(
                &images,
                &dashboard,
                ImageFormat::Bmp,
                &schedule,
                now.clone()
            )
            .await
        );
        assert!(
            !served_images_are_current(&images, &layouts, ImageFormat::Bmp, &schedule, now.clone())
                .await
        );

        images
            .publish(&LayoutName::parse("added").unwrap(), ImageFormat::Bmp, b"y")
            .await
            .unwrap();
        assert!(
            served_images_are_current(&images, &layouts, ImageFormat::Bmp, &schedule, now.clone())
                .await
        );
        // And one out of date is enough to render again.
        assert!(
            !served_images_are_current(
                &images,
                &layouts,
                ImageFormat::Bmp,
                &schedule,
                now.clone() + SignedDuration::from_hours(2)
            )
            .await
        );
    }
}

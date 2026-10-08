//! What a display needs to know after fetching the image: which render it is, whether it is
//! out of date, and when to come back. Worked out from the refresh schedule, so a cron with
//! quiet hours (e.g. `*/10 6-22 * * *`) sends the display to sleep until morning.

use std::time::Duration;

use chrono::{DateTime, Offset};
use chrono_tz::Tz;
use serde::Serialize;
use utoipa::ToSchema;

use crate::domain::models::schedule::Schedule;

/// The longest a display is told to sleep. It is also the longest the display itself will sleep, so a
/// longer figure would put the time the server expects it back after the time it really comes, and a
/// display that died during a long quiet spell (a weekend with no refreshes) wouldn't be missed until
/// the spell ended. At most one extra wake a day, and an unchanged picture is neither downloaded nor drawn.
pub const MAX_SLEEP: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Copy)]
pub struct PlanTiming {
    /// Added to the next scheduled render, so the display arrives after it has finished.
    pub wake_delay: Duration,
    /// How late a scheduled render may be before the image counts as stale.
    pub stale_grace: Duration,
}

/// What a display is told after it asks: which image is current, whether it is out of date, and when to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Plan {
    /// When the image was rendered, in seconds since 1970. Newer images have larger versions.
    #[schema(examples(1791463200))]
    pub version: u32,
    /// False when the display said it already shows this version, so it can skip the download and refresh.
    pub changed: bool,
    /// A scheduled render is long overdue, so the image is older than it should be.
    pub stale: bool,
    /// A render is due or running, so a new image is about to appear.
    pub pending: bool,
    /// How long the display should sleep before asking again.
    #[schema(examples(600))]
    pub next_seconds: u64,
    /// Time since the image was rendered.
    #[schema(examples(100))]
    pub age_seconds: u64,
    /// The zone the server works in (an IANA name such as `Europe/London`), so the display needn't be told it in
    /// its own configuration, where it could disagree.
    #[schema(examples("Europe/London"))]
    pub timezone: String,
    /// How far ahead of UTC that zone is right now, in seconds (3600 in the UK in summer). A display has no
    /// timezone database, so this is what it needs to show local time; it is right until the next clock change.
    #[schema(examples(3600))]
    pub utc_offset_seconds: i32,
}

/// The version of an image rendered at `rendered_at`: its Unix time in seconds, which fits a
/// 32-bit value until 2106.
pub fn version_of(rendered_at: DateTime<Tz>) -> u32 {
    u32::try_from(rendered_at.timestamp()).unwrap_or(u32::MAX)
}

/// When an image was rendered, as far as its time can be believed: an image stamped later than now was
/// rendered before the clock was set back, so its real age is unknown and it counts as rendered just now.
/// Taken at its word, the next render would be due that much later than any that follows, and a display
/// would be sent to sleep until then.
fn believable(rendered_at: DateTime<Tz>, now: DateTime<Tz>) -> DateTime<Tz> {
    rendered_at.min(now)
}

/// Whether the render that should follow the one served has come due, so a new one is wanted.
pub fn render_due(
    now: DateTime<Tz>,
    rendered_at: DateTime<Tz>,
    schedule: &Schedule,
) -> anyhow::Result<bool> {
    Ok(now >= schedule.next_after(believable(rendered_at, now))?)
}

/// Whether a render that was due after `rendered_at` is more than `grace` late.
pub fn is_stale(
    now: DateTime<Tz>,
    rendered_at: DateTime<Tz>,
    schedule: &Schedule,
    grace: Duration,
) -> anyhow::Result<bool> {
    let due = schedule.next_after(believable(rendered_at, now))?;
    Ok(now >= due && (now - due).to_std().unwrap_or_default() > grace)
}

/// `rendered_at` is when the served image was written; `have` is the version the display reports.
pub fn compute(
    now: DateTime<Tz>,
    rendered_at: DateTime<Tz>,
    schedule: &Schedule,
    timing: PlanTiming,
    have: Option<u32>,
) -> anyhow::Result<Plan> {
    let version = version_of(rendered_at);
    // The render that should have followed the one being served.
    let due = schedule.next_after(believable(rendered_at, now))?;
    let (pending, stale, next) = if now < due {
        (false, false, due)
    } else if (now - due).to_std().unwrap_or_default() <= timing.stale_grace {
        // Due or running: look again soon instead of waiting out a whole period.
        (true, false, now)
    } else {
        // Not rendered, long after it should have been: report it, and retry at the next slot.
        (false, true, schedule.next_after(now)?)
    };

    let wait = (next - now).num_seconds().max(0) as u64;
    Ok(Plan {
        version,
        changed: have != Some(version),
        stale,
        pending,
        next_seconds: wait
            .saturating_add(timing.wake_delay.as_secs())
            .clamp(1, MAX_SLEEP.as_secs()),
        age_seconds: (now - rendered_at).num_seconds().max(0) as u64,
        timezone: now.timezone().name().to_owned(),
        utc_offset_seconds: now.offset().fix().local_minus_utc(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    use super::*;

    const TIMING: PlanTiming = PlanTiming {
        wake_delay: Duration::from_secs(30),
        stale_grace: Duration::from_secs(300),
    };

    fn at(day: u32, h: u32, m: u32, s: u32) -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 6, day, h, m, s).unwrap()
    }

    fn plan(schedule: &str, rendered: DateTime<Tz>, now: DateTime<Tz>) -> Plan {
        let schedule = Schedule::parse_cron(schedule).unwrap();
        compute(now, rendered, &schedule, TIMING, None).unwrap()
    }

    #[test]
    fn an_image_from_the_future_counts_as_just_rendered() {
        // Rendered at 15:00, then the clock was set back to 12:00: the next slot is 12:10, not 15:10.
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let rendered = at(15, 15, 0, 0);
        let now = at(15, 12, 0, 0);

        let plan = compute(now, rendered, &schedule, TIMING, Some(version_of(rendered))).unwrap();
        assert_eq!(plan.age_seconds, 0);
        assert!(!plan.stale && !plan.pending);
        assert_eq!(
            plan.next_seconds,
            10 * 60 + 30,
            "the next slot, plus the wake delay"
        );
        // It is still the image the display shows, whatever its time says.
        assert!(!plan.changed);
        assert!(!is_stale(now, rendered, &schedule, TIMING.stale_grace).unwrap());
        assert!(!render_due(now, rendered, &schedule).unwrap());
        // Its age can't be known, so it isn't called stale either; the render history (`failing`) still says
        // if renders stop, and once the clock passes the image's own time the real age counts again.
        assert!(is_stale(at(15, 15, 40, 0), rendered, &schedule, TIMING.stale_grace).unwrap());
    }

    #[test]
    fn the_plan_says_which_zone_the_server_is_in_and_how_far_ahead_of_utc_it_is() {
        use chrono_tz::{Asia::Colombo, Australia::Lord_Howe, Pacific::Auckland};
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let in_zone = |zone: Tz, month: u32| {
            let now = zone.with_ymd_and_hms(2026, month, 15, 12, 4, 0).unwrap();
            let rendered = zone.with_ymd_and_hms(2026, month, 15, 12, 0, 0).unwrap();
            let plan = compute(now, rendered, &schedule, TIMING, None).unwrap();
            (plan.timezone, plan.utc_offset_seconds)
        };
        // The offset is the one in force at the time, so it moves with daylight saving.
        assert_eq!(in_zone(London, 1), ("Europe/London".to_owned(), 0));
        assert_eq!(in_zone(London, 7), ("Europe/London".to_owned(), 3600));
        // Whole and half hours, ahead of UTC and behind it.
        assert_eq!(
            in_zone(Colombo, 7),
            ("Asia/Colombo".to_owned(), 5 * 3600 + 1800)
        );
        assert_eq!(
            in_zone(Lord_Howe, 1),
            ("Australia/Lord_Howe".to_owned(), 11 * 3600)
        );
        assert_eq!(
            in_zone(Lord_Howe, 7),
            ("Australia/Lord_Howe".to_owned(), 10 * 3600 + 1800)
        );
        assert_eq!(
            in_zone(Auckland, 1),
            ("Pacific/Auckland".to_owned(), 13 * 3600)
        );
        assert_eq!(
            in_zone(chrono_tz::America::New_York, 1),
            ("America/New_York".to_owned(), -5 * 3600)
        );
    }

    #[test]
    fn version_is_the_render_time_and_grows() {
        let first = version_of(at(15, 12, 0, 8));
        assert_eq!(i64::from(first), at(15, 12, 0, 8).timestamp());
        assert_eq!(version_of(at(15, 12, 10, 8)), first + 600);
    }

    #[test]
    fn staleness_agrees_with_what_the_plan_reports() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let rendered = at(15, 12, 0, 8);
        for seconds in (0..40 * 60).step_by(7) {
            let now = rendered + chrono::Duration::seconds(seconds);
            let plan = compute(now, rendered, &schedule, TIMING, None).unwrap();
            let stale = is_stale(now, rendered, &schedule, TIMING.stale_grace).unwrap();
            assert_eq!(plan.stale, stale, "{seconds}s after the render");
        }
    }

    #[test]
    fn a_render_is_due_once_the_next_slot_arrives() {
        let schedule = Schedule::parse_cron("*/10 * * * *").unwrap();
        let rendered = at(15, 12, 0, 8);
        assert!(!render_due(at(15, 12, 9, 59), rendered, &schedule).unwrap());
        assert!(render_due(at(15, 12, 10, 0), rendered, &schedule).unwrap());
        assert!(render_due(at(15, 14, 0, 0), rendered, &schedule).unwrap());
    }

    #[test]
    fn a_fresh_image_sleeps_until_the_next_render_plus_the_delay() {
        let plan = plan("*/10 * * * *", at(15, 12, 0, 8), at(15, 12, 4, 0));
        assert_eq!(plan.next_seconds, 6 * 60 + 30);
        assert_eq!(plan.age_seconds, 3 * 60 + 52);
        assert!(!plan.stale && !plan.pending);
    }

    #[test]
    fn a_render_that_is_due_makes_the_display_look_again_soon() {
        let plan = plan("*/10 * * * *", at(15, 12, 0, 8), at(15, 12, 10, 3));
        assert!(plan.pending && !plan.stale);
        assert_eq!(plan.next_seconds, 30);
    }

    #[test]
    fn a_render_that_never_came_is_stale_and_retried_at_the_next_slot() {
        let plan = plan("*/10 * * * *", at(15, 12, 0, 8), at(15, 12, 16, 0));
        assert!(plan.stale && !plan.pending);
        assert_eq!(plan.next_seconds, 4 * 60 + 30);
        assert_eq!(plan.age_seconds, 15 * 60 + 52);
    }

    #[test]
    fn grace_is_measured_from_when_the_render_was_due() {
        // 12:10 was due; at 12:14:59 it is 4m59s late, still within the five minutes.
        assert!(!plan("*/10 * * * *", at(15, 12, 0, 8), at(15, 12, 14, 59)).stale);
        assert!(plan("*/10 * * * *", at(15, 12, 0, 8), at(15, 12, 15, 1)).stale);
    }

    #[test]
    fn quiet_hours_put_the_display_to_sleep_until_morning() {
        let plan = plan("*/10 6-22 * * *", at(15, 22, 50, 5), at(15, 22, 55, 0));
        // 22:55 to 06:00 is 7h05m.
        assert_eq!(plan.next_seconds, 7 * 3600 + 5 * 60 + 30);
        assert!(!plan.stale);
    }

    #[test]
    fn an_interval_schedule_counts_from_the_render() {
        let schedule = Schedule::parse_every("10m").unwrap();
        let plan = compute(at(15, 12, 4, 0), at(15, 12, 0, 0), &schedule, TIMING, None).unwrap();
        assert_eq!(plan.next_seconds, 6 * 60 + 30);
    }

    #[test]
    fn a_long_quiet_spell_is_slept_through_a_day_at_a_time() {
        // Mondays only, and it is Monday evening: the next render is nearly a week away.
        let schedule = Schedule::parse_cron("0 8 * * 1").unwrap();
        let plan = compute(at(15, 20, 0, 0), at(15, 8, 0, 5), &schedule, TIMING, None).unwrap();
        assert_eq!(plan.next_seconds, MAX_SLEEP.as_secs());
        assert!(!plan.stale && !plan.pending);
        // A wait that fits is untouched: Sunday evening to Monday at eight.
        let plan = compute(at(21, 20, 0, 0), at(15, 8, 0, 5), &schedule, TIMING, None).unwrap();
        assert_eq!(plan.next_seconds, 12 * 3600 + 30);
    }

    #[test]
    fn changed_reflects_the_version_the_display_has() {
        let schedule = Schedule::parse_every("10m").unwrap();
        let version = version_of(at(15, 12, 0, 0));
        let run = |have| {
            compute(at(15, 12, 4, 0), at(15, 12, 0, 0), &schedule, TIMING, have)
                .unwrap()
                .changed
        };
        assert!(run(None));
        assert!(run(Some(version - 600)));
        assert!(!run(Some(version)));
    }
}

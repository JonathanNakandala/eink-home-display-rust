//! What a display needs to know after fetching the image: which render it is, whether it is
//! out of date, and when to come back. Worked out from the refresh schedule, so a cron with
//! quiet hours (e.g. `*/10 6-22 * * *`) sends the display to sleep until morning.

use std::time::Duration;

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::domain::models::schedule::Schedule;

#[derive(Debug, Clone, Copy)]
pub struct PlanTiming {
    /// Added to the next scheduled render, so the display arrives after it has finished.
    pub wake_delay: Duration,
    /// How late a scheduled render may be before the image counts as stale.
    pub stale_grace: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// When the image was rendered, in seconds since 1970. Newer images have larger versions.
    pub version: u32,
    /// False when the display said it already shows this version, so it can skip the download and refresh.
    pub changed: bool,
    /// A scheduled render is long overdue, so the image is older than it should be.
    pub stale: bool,
    /// A render is due or running, so a new image is about to appear.
    pub pending: bool,
    /// How long the display should sleep before asking again.
    pub next_seconds: u64,
    /// Time since the image was rendered.
    pub age_seconds: u64,
}

/// The version of an image rendered at `rendered_at`: its Unix time in seconds, which fits a
/// 32-bit value until 2106.
pub fn version_of(rendered_at: DateTime<Local>) -> u32 {
    u32::try_from(rendered_at.timestamp()).unwrap_or(u32::MAX)
}

/// Whether the render that should follow the one served has come due, so a new one is wanted.
pub fn render_due(now: DateTime<Local>, rendered_at: DateTime<Local>, schedule: &Schedule) -> anyhow::Result<bool> {
    Ok(now >= schedule.next_after(rendered_at)?)
}

/// Whether a render that was due after `rendered_at` is more than `grace` late.
pub fn is_stale(
    now: DateTime<Local>,
    rendered_at: DateTime<Local>,
    schedule: &Schedule,
    grace: Duration,
) -> anyhow::Result<bool> {
    let due = schedule.next_after(rendered_at)?;
    Ok(now >= due && (now - due).to_std().unwrap_or_default() > grace)
}

/// `rendered_at` is when the served image was written; `have` is the version the display reports.
pub fn compute(
    now: DateTime<Local>,
    rendered_at: DateTime<Local>,
    schedule: &Schedule,
    timing: PlanTiming,
    have: Option<u32>,
) -> anyhow::Result<Plan> {
    let version = version_of(rendered_at);
    // The render that should have followed the one being served.
    let due = schedule.next_after(rendered_at)?;
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
        next_seconds: (wait + timing.wake_delay.as_secs()).max(1),
        age_seconds: (now - rendered_at).num_seconds().max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const TIMING: PlanTiming =
        PlanTiming { wake_delay: Duration::from_secs(30), stale_grace: Duration::from_secs(300) };

    fn at(day: u32, h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, day, h, m, s).unwrap()
    }

    fn plan(schedule: &str, rendered: DateTime<Local>, now: DateTime<Local>) -> Plan {
        let schedule = Schedule::parse_cron(schedule).unwrap();
        compute(now, rendered, &schedule, TIMING, None).unwrap()
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
    fn changed_reflects_the_version_the_display_has() {
        let schedule = Schedule::parse_every("10m").unwrap();
        let version = version_of(at(15, 12, 0, 0));
        let run = |have| compute(at(15, 12, 4, 0), at(15, 12, 0, 0), &schedule, TIMING, have).unwrap().changed;
        assert!(run(None));
        assert!(run(Some(version - 600)));
        assert!(!run(Some(version)));
    }
}

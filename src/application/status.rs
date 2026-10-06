//! What the server can say about its own health: when it last rendered, what went wrong if it
//! didn't, and how the data sources were doing. Served as `/status`, and reduced to a pass or
//! fail for `/healthz`.
//!
//! The one failure that makes the service unhealthy is a stale image: a scheduled render long
//! overdue (the same rule `/plan` uses, so the two can't disagree). A source being down is
//! shown, but the dashboard is still updating, so it is not a failed health check.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Local};
use serde::Serialize;

use super::devices::DeviceStatus;
use super::plan::{is_stale, version_of, PlanTiming};
use crate::domain::models::freshness::format_age;
use crate::domain::models::render_report::{RenderReport, SourceReport};
use crate::domain::models::schedule::Schedule;
use crate::domain::services::render_observer::RenderObserver;

/// Long enough for a render error to be recognisable, short enough for a one-line log or a page.
const MAX_ERROR_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// No image yet, and the service has only just started.
    Starting,
    Ok,
    /// Rendering, but some source is shown from old data or left off.
    Degraded,
    /// The last render failed, though the image is not yet old enough to be stale.
    Failing,
    /// A scheduled render is long overdue, or there is still no image well after start-up.
    Stale,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub state: Health,
    pub rendering: bool,
    pub image: Option<ImageStatus>,
    pub last_success: Option<Moment>,
    pub last_failure: Option<FailureStatus>,
    pub consecutive_failures: u32,
    /// How each source was on the last successful render.
    pub sources: Vec<SourceReport>,
    /// The displays that have checked in, with their battery and whether any has gone quiet.
    pub devices: Vec<DeviceStatus>,
    pub next_render: Option<DateTime<Local>>,
    /// The refresh schedule in words, one line per cron expression.
    pub schedule: Vec<String>,
    pub uptime_seconds: u64,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ImageStatus {
    pub rendered_at: DateTime<Local>,
    pub age_seconds: u64,
    /// The same number `/plan` reports.
    pub version: u32,
}

#[derive(Debug, Serialize)]
pub struct Moment {
    pub at: DateTime<Local>,
    pub age_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct FailureStatus {
    pub at: DateTime<Local>,
    pub age_seconds: u64,
    pub error: String,
}

impl Status {
    /// Whether a monitor should treat the service as working.
    pub fn is_healthy(&self) -> bool {
        self.state != Health::Stale
    }

    /// One line for `/healthz`.
    pub fn summary(&self) -> String {
        match (self.state, &self.image) {
            (Health::Stale, None) => "stale: no image has been rendered".to_owned(),
            (Health::Stale, Some(image)) => format!(
                "stale: the image is {} old",
                format_age(chrono::Duration::try_seconds(image.age_seconds.try_into().unwrap_or(i64::MAX)).unwrap_or(chrono::Duration::zero()))
            ),
            (Health::Starting, _) => "starting".to_owned(),
            (Health::Ok, _) => "ok".to_owned(),
            (Health::Degraded, _) => "degraded".to_owned(),
            (Health::Failing, _) => "failing".to_owned(),
        }
    }
}

struct Success {
    at: DateTime<Local>,
    report: RenderReport,
}

struct Failure {
    at: DateTime<Local>,
    error: String,
}

#[derive(Default)]
struct Record {
    rendering: bool,
    last_success: Option<Success>,
    last_failure: Option<Failure>,
    consecutive_failures: u32,
}

/// The render history. Shared between the render loop, which writes it, and the server, which
/// reads it. Times are passed in, so the logic is the same in tests as in use.
pub struct StatusBoard {
    started: DateTime<Local>,
    record: Mutex<Record>,
}

impl StatusBoard {
    pub fn new(started: DateTime<Local>) -> Arc<Self> {
        Arc::new(Self { started, record: Mutex::default() })
    }

    // The record is plain data and each update leaves it consistent, so a panic elsewhere while the
    // lock was held is no reason to stop reporting.
    fn record(&self) -> std::sync::MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The status as of `now`. `rendered_at` is when the image being served was written, if there is one.
    pub fn status(
        &self,
        now: DateTime<Local>,
        rendered_at: Option<DateTime<Local>>,
        schedule: &Schedule,
        timing: PlanTiming,
    ) -> anyhow::Result<Status> {
        let record = self.record();
        let uptime = (now - self.started).max(chrono::Duration::zero());
        let age = |at: DateTime<Local>| (now - at).num_seconds().max(0).unsigned_abs();

        let state = match rendered_at {
            None if uptime.to_std().unwrap_or_default() <= timing.stale_grace => Health::Starting,
            None => Health::Stale,
            Some(rendered_at) if is_stale(now, rendered_at, schedule, timing.stale_grace)? => Health::Stale,
            Some(_) if record.consecutive_failures > 0 => Health::Failing,
            Some(_) if record.last_success.as_ref().is_some_and(|s| s.report.is_degraded()) => Health::Degraded,
            Some(_) => Health::Ok,
        };

        Ok(Status {
            state,
            rendering: record.rendering,
            image: rendered_at.map(|rendered_at| ImageStatus {
                rendered_at,
                age_seconds: age(rendered_at),
                version: version_of(rendered_at),
            }),
            last_success: record.last_success.as_ref().map(|s| Moment { at: s.at, age_seconds: age(s.at) }),
            last_failure: record.last_failure.as_ref().map(|f| FailureStatus {
                at: f.at,
                age_seconds: age(f.at),
                error: f.error.clone(),
            }),
            consecutive_failures: record.consecutive_failures,
            sources: record.last_success.as_ref().map(|s| s.report.sources.clone()).unwrap_or_default(),
            devices: Vec::new(),
            next_render: schedule.next_after(now).ok(),
            schedule: schedule.describe(),
            uptime_seconds: uptime.num_seconds().unsigned_abs(),
            version: env!("CARGO_PKG_VERSION"),
        })
    }
}

impl RenderObserver for StatusBoard {
    fn render_started(&self) {
        self.record().rendering = true;
    }

    fn render_succeeded(&self, at: DateTime<Local>, report: &RenderReport) {
        let mut record = self.record();
        record.rendering = false;
        record.consecutive_failures = 0;
        record.last_success = Some(Success { at, report: report.clone() });
    }

    fn render_failed(&self, at: DateTime<Local>, error: &anyhow::Error) {
        let mut record = self.record();
        record.rendering = false;
        record.consecutive_failures = record.consecutive_failures.saturating_add(1);
        record.last_failure = Some(Failure { at, error: one_line(&format!("{error:#}")) });
    }
}

/// An error message as one line of bounded length: newlines folded, and cut on a character boundary.
fn one_line(message: &str) -> String {
    let folded = message.split_whitespace().collect::<Vec<_>>().join(" ");
    match folded.char_indices().nth(MAX_ERROR_CHARS) {
        Some((end, _)) => format!("{}…", &folded[..end]),
        None => folded,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::anyhow;
    use chrono::TimeZone;

    use super::*;
    use crate::domain::models::render_report::SourceState;

    const TIMING: PlanTiming =
        PlanTiming { wake_delay: Duration::from_secs(30), stale_grace: Duration::from_secs(300) };

    fn at(h: u32, m: u32, s: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, s).unwrap()
    }

    fn schedule() -> Schedule {
        Schedule::parse_cron("*/10 * * * *").unwrap()
    }

    fn health(board: &StatusBoard, now: DateTime<Local>, rendered_at: Option<DateTime<Local>>) -> Health {
        board.status(now, rendered_at, &schedule(), TIMING).unwrap().state
    }

    fn report(state: SourceState) -> RenderReport {
        RenderReport { sources: vec![SourceReport { name: "weather".into(), state }] }
    }

    #[test]
    fn a_new_service_with_no_image_is_starting_until_the_grace_runs_out() {
        let board = StatusBoard::new(at(12, 0, 0));
        assert_eq!(health(&board, at(12, 0, 30), None), Health::Starting);
        assert_eq!(health(&board, at(12, 5, 0), None), Health::Starting);
        assert_eq!(health(&board, at(12, 5, 1), None), Health::Stale);
    }

    #[test]
    fn a_current_image_after_clean_renders_is_ok() {
        let board = StatusBoard::new(at(11, 0, 0));
        board.render_started();
        board.render_succeeded(at(12, 0, 5), &report(SourceState::Fresh));
        assert_eq!(health(&board, at(12, 4, 0), Some(at(12, 0, 5))), Health::Ok);
    }

    #[test]
    fn a_source_that_is_down_makes_it_degraded_but_still_healthy() {
        let board = StatusBoard::new(at(11, 0, 0));
        board.render_succeeded(at(12, 0, 5), &report(SourceState::Unavailable { reason: "key rejected".into() }));

        let status = board.status(at(12, 4, 0), Some(at(12, 0, 5)), &schedule(), TIMING).unwrap();
        assert_eq!(status.state, Health::Degraded);
        assert!(status.is_healthy());
        assert_eq!(status.summary(), "degraded");
        assert_eq!(status.sources.len(), 1);
    }

    #[test]
    fn a_failed_render_is_failing_until_the_image_is_stale_and_a_success_clears_it() {
        let board = StatusBoard::new(at(11, 0, 0));
        board.render_succeeded(at(12, 0, 5), &report(SourceState::Fresh));
        board.render_failed(at(12, 10, 20), &anyhow!("Chrome did not start"));

        // 12:10 was due and failed; at 12:12 the image is late but within the grace.
        assert_eq!(health(&board, at(12, 12, 0), Some(at(12, 0, 5))), Health::Failing);
        // Past the grace the image is simply stale, whatever the cause.
        let stale = board.status(at(12, 16, 0), Some(at(12, 0, 5)), &schedule(), TIMING).unwrap();
        assert_eq!(stale.state, Health::Stale);
        assert!(!stale.is_healthy());
        assert_eq!(stale.summary(), "stale: the image is 15 min old");
        assert_eq!(stale.consecutive_failures, 1);
        assert_eq!(stale.last_failure.as_ref().unwrap().error, "Chrome did not start");

        board.render_succeeded(at(12, 20, 3), &report(SourceState::Fresh));
        let ok = board.status(at(12, 21, 0), Some(at(12, 20, 3)), &schedule(), TIMING).unwrap();
        assert_eq!(ok.state, Health::Ok);
        assert_eq!(ok.consecutive_failures, 0);
        // What went wrong last is kept for reference.
        assert!(ok.last_failure.is_some());
    }

    #[test]
    fn rendering_is_true_only_while_a_render_runs() {
        let board = StatusBoard::new(at(11, 0, 0));
        let rendering = |board: &StatusBoard| board.status(at(12, 0, 0), None, &schedule(), TIMING).unwrap().rendering;
        assert!(!rendering(&board));
        board.render_started();
        assert!(rendering(&board));
        board.render_failed(at(12, 0, 1), &anyhow!("x"));
        assert!(!rendering(&board));
    }

    #[test]
    fn the_status_reports_ages_the_next_render_and_the_image_version() {
        let board = StatusBoard::new(at(11, 0, 0));
        board.render_succeeded(at(12, 0, 5), &report(SourceState::Fresh));
        let status = board.status(at(12, 4, 5), Some(at(12, 0, 5)), &schedule(), TIMING).unwrap();

        assert_eq!(status.image.as_ref().unwrap().age_seconds, 240);
        assert_eq!(status.image.as_ref().unwrap().version, version_of(at(12, 0, 5)));
        assert_eq!(status.last_success.as_ref().unwrap().age_seconds, 240);
        assert_eq!(status.next_render, Some(at(12, 10, 0)));
        assert_eq!(status.schedule, ["*/10 * * * *  At every 10 minutes."]);
        assert_eq!(status.uptime_seconds, 3845);
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn error_text_is_folded_to_one_bounded_line() {
        assert_eq!(one_line("first\nsecond   third"), "first second third");
        let long = "é".repeat(MAX_ERROR_CHARS + 50);
        let cut = one_line(&long);
        assert_eq!(cut.chars().count(), MAX_ERROR_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_poisoned_lock_does_not_stop_reporting() {
        let board = StatusBoard::new(at(11, 0, 0));
        let shared = Arc::clone(&board);
        let _ = std::thread::spawn(move || {
            let _guard = shared.record();
            panic!("a panic while the record is locked");
        })
        .join();

        board.render_succeeded(at(12, 0, 0), &RenderReport::default());
        assert!(board.status(at(12, 1, 0), Some(at(12, 0, 0)), &schedule(), TIMING).unwrap().is_healthy());
    }
}

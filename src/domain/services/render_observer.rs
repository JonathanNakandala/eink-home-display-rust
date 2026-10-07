use chrono::{DateTime};
use chrono_tz::Tz;

use crate::domain::models::render_report::RenderReport;

/// Told how each render goes, so whatever reports on or reacts to renders (a health page,
/// a button waiting for its render, a restart guard) needn't be wired in around the run.
///
/// Called from the render itself, so keep each one quick.
pub trait RenderObserver: Send + Sync {
    fn render_started(&self);
    fn render_succeeded(&self, at: DateTime<Tz>, report: &RenderReport);
    fn render_failed(&self, at: DateTime<Tz>, error: &anyhow::Error);
}

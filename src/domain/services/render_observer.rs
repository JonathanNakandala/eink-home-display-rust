use chrono::DateTime;
use chrono_tz::Tz;

use crate::domain::models::render_report::RenderReport;

/// Told how each render goes, so whatever reports on or reacts to renders (a health page,
/// a button waiting for its render, a restart guard) needn't be wired in around the run.
///
/// Called from the render itself, so keep each one quick.
pub trait RenderObserver: Send + Sync {
    fn render_started(&self);
    /// The display's pictures are drawn and kept, and the outputs besides it (a web page's own) are about to be: what a display
    /// that asked for a render is waiting for. Said before the run ends, which is when `render_succeeded` is. A run that fails
    /// before the display is done never says it.
    fn display_updated(&self) {}
    fn render_succeeded(&self, at: DateTime<Tz>, report: &RenderReport);
    fn render_failed(&self, at: DateTime<Tz>, error: &anyhow::Error);
}

//! For tests that check what is logged: records every log line so a test can look for a message, or for
//! the absence of one. The logger is process-wide and tests run side by side, so a test looks only for
//! lines that mention something unique to it (a device name of its own).

use std::sync::{Mutex, Once, PoisonError};

use log::{LevelFilter, Log, Metadata, Record};

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INSTALL: Once = Once::new();

struct Capture;

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        LINES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}", record.level(), record.args()));
    }

    fn flush(&self) {}
}

/// Starts recording. Safe to call from every test that needs it.
pub fn install() {
    INSTALL.call_once(|| {
        // Another logger may already be set in this process; then nothing is recorded and a test that
        // depends on it fails, which is the right way to find out.
        let _ = log::set_logger(&Capture);
        log::set_max_level(LevelFilter::Debug);
    });
}

/// Every line logged so far that contains `needle`.
pub fn lines_containing(needle: &str) -> Vec<String> {
    LINES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|line| line.contains(needle))
        .cloned()
        .collect()
}

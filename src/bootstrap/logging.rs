//! Where the program's log lines (and, if asked, its traces) go.
//!
//! - Text, the default: for a person at a terminal. Coloured only when stdout is a terminal and `NO_COLOR` is not set,
//!   so a journal does not fill with escape codes.
//! - `LOG_FORMAT=json`: one JSON object per line, the event's fields at the top level (`level`, `message`, `target`, and
//!   any field the call gave), plus those of the span it happened in. For Loki (`| json`) or any log shipper.
//!
//! `RUST_LOG` sets what is shown (`info`, `home_display_server=debug`, ...). With `--features otel`, setting
//! `OTEL_EXPORTER_OTLP_ENDPOINT` (e.g. `http://localhost:4318`, an Alloy `otelcol.receiver.otlp`) also exports the spans
//! and the logs over OTLP; see `telemetry`.

use std::io::IsTerminal;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};

use super::telemetry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

impl Format {
    /// `None` for a value that is neither, so the caller can say so once logging works.
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Keep this until the program ends: dropping it sends what the exporters still hold.
#[must_use = "dropping this at once would stop the telemetry export before anything is sent"]
pub struct LogGuard {
    _telemetry: telemetry::Guard,
}

/// Starts logging. `RUST_LOG` overrides `default_filter` (e.g. "debug"), and `LOG_FORMAT=json` writes JSON lines.
pub fn init_logging(default_filter: &str) -> LogGuard {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let requested = std::env::var("LOG_FORMAT").unwrap_or_default();
    let format = Format::parse(&requested);

    let output = match format.unwrap_or(Format::Text) {
        Format::Json => fmt::layer()
            .with_ansi(false)
            .with_target(true)
            .with_file(true)
            .with_line_number(true)
            .json()
            .flatten_event(true)
            .boxed(),
        Format::Text => fmt::layer()
            .with_ansi(std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
            .boxed(),
    };

    let (telemetry_layers, guard) = telemetry::init();
    tracing_subscriber::registry()
        .with(env_filter)
        .with(output)
        .with(telemetry_layers)
        .init();

    if format.is_none() {
        tracing::warn!(
            log_format = requested,
            "LOG_FORMAT is neither \"text\" nor \"json\"; using text"
        );
    }
    guard.report();
    LogGuard { _telemetry: guard }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_is_text_unless_json_is_asked_for() {
        assert_eq!(Format::parse(""), Some(Format::Text));
        assert_eq!(Format::parse("text"), Some(Format::Text));
        assert_eq!(Format::parse(" JSON "), Some(Format::Json));
        assert_eq!(Format::parse("logfmt"), None);
    }
}

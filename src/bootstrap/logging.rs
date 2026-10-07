use tracing_subscriber::{EnvFilter, fmt};

/// Logs to the terminal. `RUST_LOG` overrides `default_filter` (e.g. "debug").
pub fn init_logging(default_filter: &str) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    fmt().with_env_filter(env_filter).init();
}

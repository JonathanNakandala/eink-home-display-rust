//! Optional OpenTelemetry export, for Grafana Alloy (`otelcol.receiver.otlp`) or any OTLP collector.
//!
//! Built only with `--features otel`, and then only active when `OTEL_EXPORTER_OTLP_ENDPOINT` (or the `_TRACES_` /
//! `_LOGS_` forms of it) is set, so a package built with it behaves as before until asked. The rest is the standard
//! environment: `OTEL_SERVICE_NAME` (default `home-display-server`), `OTEL_RESOURCE_ATTRIBUTES`,
//! `OTEL_EXPORTER_OTLP_HEADERS`; `OTEL_EXPORTER_OTLP_PROTOCOL` is not used: it is always OTLP over HTTP (port 4318).
//! `OTEL_TRACES_EXPORTER=none` and `OTEL_LOGS_EXPORTER=none` turn off one of the two, e.g. the logs when the journal already
//! ships them (the SDK itself ignores these, so they are read here).
//!
//! What is sent: every `tracing` span as a trace span (a refresh run, a layout drawn, a source fetched), and every event as
//! a log record carrying the span it happened in. Metrics are not sent this way; `/metrics` serves those to a scraper.

#[cfg(feature = "otel")]
pub use enabled::{Guard, init};

#[cfg(not(feature = "otel"))]
pub use disabled::{Guard, init};

#[cfg(not(feature = "otel"))]
mod disabled {
    use tracing_subscriber::Layer;

    pub struct Guard;

    impl Guard {
        pub fn report(&self) {
            if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_some() {
                tracing::warn!(
                    "OTEL_EXPORTER_OTLP_ENDPOINT is set, but this build has no OpenTelemetry (build with --features otel)"
                );
            }
        }
    }

    pub fn init<S>() -> (Vec<Box<dyn Layer<S> + Send + Sync>>, Guard)
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a> + Send + Sync,
    {
        (Vec::new(), Guard)
    }
}

#[cfg(feature = "otel")]
mod enabled {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_otlp::{LogExporter, SpanExporter};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::registry::LookupSpan;

    const SERVICE: &str = "home-display-server";

    /// Flushes and stops the exporters when dropped.
    pub struct Guard {
        traces: Option<SdkTracerProvider>,
        logs: Option<SdkLoggerProvider>,
        failure: Option<String>,
    }

    impl Guard {
        /// Says, once logging works, what `init` did.
        pub fn report(&self) {
            match (&self.traces, &self.logs, &self.failure) {
                (_, _, Some(why)) => tracing::warn!(error = %why, "Not exporting over OTLP"),
                (Some(_), Some(_), None) => tracing::info!("Exporting traces and logs over OTLP"),
                (Some(_), None, None) => tracing::info!("Exporting traces over OTLP"),
                (None, Some(_), None) => tracing::info!("Exporting logs over OTLP"),
                (None, None, None) => {}
            }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // The exporters' own trouble can only go to stderr: logging may be what is being flushed.
            if let Some(Err(e)) = self.traces.take().map(|traces| traces.shutdown()) {
                eprintln!("Could not flush the traces: {e}");
            }
            if let Some(Err(e)) = self.logs.take().map(|logs| logs.shutdown()) {
                eprintln!("Could not flush the logs: {e}");
            }
        }
    }

    fn wanted() -> bool {
        [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    }

    /// Whether the standard `OTEL_<KIND>_EXPORTER` variable turns that kind of export off (the SDK does not read it).
    fn switched_off(variable: &str) -> bool {
        std::env::var(variable).is_ok_and(|value| value.trim().eq_ignore_ascii_case("none"))
    }

    fn resource() -> Resource {
        let mut builder = Resource::builder().with_attribute(opentelemetry::KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION"),
        ));
        // `Resource::builder` reads OTEL_SERVICE_NAME and OTEL_RESOURCE_ATTRIBUTES; this is only the fallback name.
        if std::env::var_os("OTEL_SERVICE_NAME").is_none() {
            builder = builder.with_service_name(SERVICE);
        }
        builder.build()
    }

    fn trace_provider(resource: Resource) -> Result<SdkTracerProvider, String> {
        let exporter = SpanExporter::builder()
            .with_http()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build())
    }

    fn log_provider(resource: Resource) -> Result<SdkLoggerProvider, String> {
        let exporter = LogExporter::builder()
            .with_http()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build())
    }

    /// The exporter's own HTTP client logs as it sends; exporting those would make each batch cause the next.
    fn not_the_exporter(metadata: &tracing::Metadata<'_>) -> bool {
        const OWN: [&str; 7] = [
            "opentelemetry",
            "hyper",
            "hyper_util",
            "reqwest",
            "h2",
            "tower",
            "rustls",
        ];
        !OWN.iter().any(|prefix| {
            metadata.target() == *prefix
                || metadata
                    .target()
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with("::"))
        })
    }

    pub fn init<S>() -> (Vec<Box<dyn Layer<S> + Send + Sync>>, Guard)
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        let mut guard = Guard {
            traces: None,
            logs: None,
            failure: None,
        };
        let mut layers: Vec<Box<dyn Layer<S> + Send + Sync>> = Vec::new();
        if !wanted() {
            return (layers, guard);
        }
        if !switched_off("OTEL_TRACES_EXPORTER") {
            match trace_provider(resource()) {
                Ok(provider) => {
                    layers.push(
                        tracing_opentelemetry::layer()
                            .with_tracer(provider.tracer(SERVICE))
                            .with_filter(filter_fn(not_the_exporter))
                            .boxed(),
                    );
                    guard.traces = Some(provider);
                }
                Err(why) => guard.failure = Some(why),
            }
        }
        if !switched_off("OTEL_LOGS_EXPORTER") {
            match log_provider(resource()) {
                Ok(provider) => {
                    layers.push(
                        OpenTelemetryTracingBridge::new(&provider)
                            .with_filter(filter_fn(not_the_exporter))
                            .boxed(),
                    );
                    guard.logs = Some(provider);
                }
                Err(why) => guard.failure = Some(why),
            }
        }
        (layers, guard)
    }
}

use std::str::FromStr;

use anyhow::Result;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SpanExporter;
use tracing::subscriber::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::{Filter, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

/// Configures [`setup_tracing`] to enable or disable the open-telemetry exporter.
#[derive(Clone)]
pub struct TracingConfig {
    /// OpenTelemetry configuration
    pub otel: OpenTelemetry,
    /// Export data JSON-formatted
    pub json_format: bool,
}

/// OpenTelemetry configuration
#[derive(Clone)]
pub enum OpenTelemetry {
    /// Enable OpenTelemetry export
    Enabled {
        /// Endpoint
        endpoint: String,
    },
    /// Disable OpenTelemetry
    Disabled,
}

impl TracingConfig {
    /// Create a new tracing configuration with explicit parameters.
    pub fn new(enable_otel: bool, otel_endpoint: String) -> Self {
        let otel = if enable_otel {
            OpenTelemetry::Enabled { endpoint: otel_endpoint }
        } else {
            OpenTelemetry::Disabled
        };

        TracingConfig {
            otel,
            json_format: std::env::var("JSON_LOGGING")
                .unwrap_or_else(|_| "false".to_string())
                .parse()
                .unwrap_or(false),
        }
    }
}

impl OpenTelemetry {
    /// Is OpenTelemetry enabled
    pub fn is_enabled(&self) -> bool {
        matches!(self, OpenTelemetry::Enabled { .. })
    }
}

/// Initializes tracing to stdout and optionally an open-telemetry exporter.
///
/// Trace filtering defaults to `INFO` and can be configured using the conventional `RUST_LOG`
/// environment variable.
///
/// The open-telemetry configuration is controlled via environment variables as defined in the
/// [specification](https://github.com/open-telemetry/opentelemetry-specification/blob/main/specification/protocol/exporter.md#opentelemetry-protocol-exporter)
pub fn setup_tracing(cfg: TracingConfig) -> Result<()> {
    if cfg.otel.is_enabled() {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

        // Setup metrics export if OTEL is enabled
        setup_metrics_export(&cfg.otel)?;
    }

    // Note: open-telemetry requires a tokio-runtime, so this _must_ be lazily evaluated (aka not
    // `then_some`) to avoid crashing sync callers (with OpenTelemetry::Disabled set). Examples of
    // such callers are tests with logging enabled.
    let otel_layer = {
        if let OpenTelemetry::Enabled { endpoint } = cfg.otel {
            let exporter_builder =
                opentelemetry_otlp::SpanExporter::builder().with_tonic().with_endpoint(endpoint);

            match exporter_builder.build() {
                Ok(exporter) => {
                    Some(open_telemetry_layer(exporter, "miden-note-transport-node".to_string()))
                },
                Err(_) => None,
            }
        } else {
            None
        }
    };

    let subscriber = Registry::default()
        .with(stdout_layer(cfg.json_format).with_filter(env_or_default_filter()))
        .with(otel_layer.with_filter(env_or_default_filter()));

    tracing::subscriber::set_global_default(subscriber).map_err(Into::into)
}

/// Setup OpenTelemetry metrics export using the proper SDK API
fn setup_metrics_export(otel_cfg: &OpenTelemetry) -> Result<()> {
    if let OpenTelemetry::Enabled { endpoint } = otel_cfg {
        // Configure OTLP metrics pipeline
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()?;

        let provider = SdkMeterProvider::builder()
            .with_reader(
                PeriodicReader::builder(exporter)
                    .with_interval(std::time::Duration::from_secs(5)) // Push interval
                    .build(),
            )
            .build();

        // Set the meter provider globally
        opentelemetry::global::set_meter_provider(provider);
    }

    Ok(())
}

/// Initializes tracing to a test exporter.
///
/// Allows trace content to be inspected via the returned receiver.
///
/// All tests that use this function must be annotated with `#[serial(open_telemetry_tracing)]`.
/// This forces serialization of all such tests. Otherwise, the tested spans could
/// be interleaved during runtime. Also, the global exporter could be re-initialized in
/// the middle of a concurrently running test.
#[cfg(test)]
pub fn setup_test_tracing() -> Result<(
    tokio::sync::mpsc::UnboundedReceiver<opentelemetry_sdk::trace::SpanData>,
    tokio::sync::mpsc::UnboundedReceiver<()>,
)> {
    let (exporter, rx_export, rx_shutdown) =
        opentelemetry_sdk::testing::trace::new_tokio_test_exporter();

    let otel_layer = open_telemetry_layer(exporter, "test-service".to_string());
    let subscriber = Registry::default()
        .with(stdout_layer(true).with_filter(env_or_default_filter()))
        .with(otel_layer.with_filter(env_or_default_filter()));
    tracing::subscriber::set_global_default(subscriber)?;
    Ok((rx_export, rx_shutdown))
}

fn open_telemetry_layer<S>(
    exporter: impl SpanExporter + 'static,
    service_name: String,
) -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync + 'static>
where
    S: Subscriber + Sync + Send,
    for<'a> S: tracing_subscriber::registry::LookupSpan<'a>,
{
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer(service_name);

    // Set the tracer provider globally
    opentelemetry::global::set_tracer_provider(provider);

    OpenTelemetryLayer::new(tracer).boxed()
}

fn stdout_layer<S>(
    json_logging: bool,
) -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync + 'static>
where
    S: Subscriber,
    for<'a> S: tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::fmt::format::FmtSpan;

    if json_logging {
        tracing_subscriber::fmt::layer()
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .pretty()
            .compact()
            .with_level(true)
            .with_file(true)
            .with_line_number(true)
            .with_target(true)
            .with_span_events(FmtSpan::CLOSE)
            .boxed()
    }
}

/// Creates a filter from the `RUST_LOG` env var with a default of `INFO` if unset.
///
/// # Panics
///
/// Panics if `RUST_LOG` fails to parse.
fn env_or_default_filter<S>() -> Box<dyn Filter<S> + Send + Sync + 'static> {
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::filter::{FilterExt, Targets};

    // `tracing` does not allow differentiating between invalid and missing env var so we manually
    // do this instead. The alternative is to silently ignore parsing errors which I think is worse.
    match std::env::var(EnvFilter::DEFAULT_ENV) {
        Ok(rust_log) => FilterExt::boxed(
            EnvFilter::from_str(&rust_log)
                .expect("RUST_LOG should contain a valid filter configuration"),
        ),
        Err(std::env::VarError::NotUnicode(_)) => panic!("RUST_LOG contained non-unicode"),
        Err(std::env::VarError::NotPresent) => {
            // Default level is INFO, and additionally enable logs from axum extractor rejections.
            FilterExt::boxed(
                Targets::new()
                    .with_default(LevelFilter::INFO)
                    .with_target("axum::rejection", LevelFilter::TRACE),
            )
        },
    }
}

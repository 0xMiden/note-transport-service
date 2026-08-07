use std::mem::ManuallyDrop;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

/// Transport metrics using OpenTelemetry metrics
///
/// If [`Metrics`] needs to be shared, cloning is recommended.
#[derive(Debug, Clone)]
pub struct Metrics {
    /// [`crate::node::grpc::GrpcServer`] metrics
    pub grpc: MetricsGrpc,
    /// [`crate::database::Database`] metrics
    pub db: MetricsDatabase,
}

/// [`crate::node::grpc::GrpcServer`] metrics
#[derive(Debug, Clone)]
pub struct MetricsGrpc {
    // -- gRPC
    // send_note()
    send_note_count: Counter<u64>,
    send_note_duration: Histogram<f64>,
    send_note_note_size: Histogram<u64>,
    // fetch_notes()
    fetch_notes_count: Counter<u64>,
    fetch_notes_duration: Histogram<f64>,
    fetch_notes_replied_notes_number: Histogram<u64>,
    fetch_notes_replied_notes_size: Histogram<u64>,
}

/// [`crate::database::Database`] metrics
#[derive(Debug, Clone)]
pub struct MetricsDatabase {
    // -- DB
    // store_note()
    store_note_count: Counter<u64>,
    store_note_duration: Histogram<f64>,
    // fetch_notes()
    fetch_notes_count: Counter<u64>,
    fetch_notes_duration: Histogram<f64>,
    // legacy cursor reset (pre-seq-migration clients)
    fetch_notes_legacy_cursor_reset_count: Counter<u64>,
    // Maintenance
    maintenance_cleanup_notes_count: Counter<u64>,
    maintenance_cleanup_notes_duration: Histogram<f64>,
}

impl Metrics {
    /// Create a new instance of `Metrics`
    pub fn new(meter: &Meter) -> Self {
        let grpc = MetricsGrpc::new(meter);
        let db = MetricsDatabase::new(meter);
        Self { grpc, db }
    }
}

impl MetricsGrpc {
    /// Create a new instance of `MetricsGrpc`
    pub fn new(meter: &Meter) -> Self {
        let send_note_count = meter
            .u64_counter("grpc_send_note_count")
            .with_description("Total number of gRPC send_note() requests")
            .build();

        let send_note_duration = meter
            .f64_histogram("grpc_send_note_duration")
            .with_description("Duration of gRPC send_note() requests in seconds")
            .with_unit("s")
            .build();

        let send_note_note_size = meter
            .u64_histogram("grpc_send_note_note_size")
            .with_description("Size of incoming note in send_note() requests in bytes")
            .with_unit("B")
            .build();

        let fetch_notes_count = meter
            .u64_counter("grpc_fetch_notes_count")
            .with_description("Total number of gRPC fetch_notes() requests")
            .build();

        let fetch_notes_duration = meter
            .f64_histogram("grpc_fetch_notes_duration")
            .with_description("Duration of gRPC fetch_notes() requests in seconds")
            .with_unit("s")
            .build();

        let fetch_notes_replied_notes_number = meter
            .u64_histogram("grpc_fetch_notes_replied_notes_number")
            .with_description("Number of replied notes per gRPC fetch_notes() request")
            .build();

        let fetch_notes_replied_notes_size = meter
            .u64_histogram("grpc_fetch_notes_replied_notes_size")
            .with_description("Total size of replied notes per gRPC fetch_notes() request in bytes")
            .with_unit("B")
            .build();

        Self {
            send_note_count,
            send_note_duration,
            send_note_note_size,
            fetch_notes_count,
            fetch_notes_duration,
            fetch_notes_replied_notes_number,
            fetch_notes_replied_notes_size,
        }
    }

    /// Measure a send-note request
    ///
    /// Increases the request counter, records note size, and measures request duration.
    pub fn grpc_send_note_request(&self, size_b: u64) -> RequestTimer<'_> {
        let operation = "grpc.send_note.request";

        self.send_note_note_size
            .record(size_b, &[KeyValue::new("operation", operation.to_string())]);

        let counter = &self.send_note_count;
        let histogram = &self.send_note_duration;
        request_count_measure(operation, counter, histogram)
    }

    /// Measure a fetch-notes request
    ///
    /// Increases the request counter and measures request duration.
    pub fn grpc_fetch_notes_request(&self) -> RequestTimer<'_> {
        let operation = "grpc.fetch_notes";
        let counter = &self.fetch_notes_count;
        let histogram = &self.fetch_notes_duration;

        request_count_measure(operation, counter, histogram)
    }

    /// Measure a fetch-notes response
    ///
    /// Records number and size of replied notes.
    pub fn grpc_fetch_notes_response(&self, number: u64, size_b: u64) {
        let operation = "grpc.fetch_notes.response";

        self.fetch_notes_replied_notes_number
            .record(number, &[KeyValue::new("operation", operation.to_string())]);
        self.fetch_notes_replied_notes_size
            .record(size_b, &[KeyValue::new("operation", operation.to_string())]);
    }
}

impl MetricsDatabase {
    /// Create a new instance of `MetricsDatabase`
    pub fn new(meter: &Meter) -> Self {
        let store_note_count = meter
            .u64_counter("db_store_note_count")
            .with_description("Total number of DB store_note() requests")
            .build();

        let store_note_duration = meter
            .f64_histogram("db_store_note_duration")
            .with_description("Duration of DB store_note() requests in seconds")
            .with_unit("s")
            .build();

        let fetch_notes_count = meter
            .u64_counter("db_fetch_notes_count")
            .with_description("Total number of DB fetch_notes() requests")
            .build();

        let fetch_notes_duration = meter
            .f64_histogram("db_fetch_notes_duration")
            .with_description("Duration of dB fetch_notes() requests in seconds")
            .with_unit("s")
            .build();

        let fetch_notes_legacy_cursor_reset_count = meter
            .u64_counter("db_fetch_notes_legacy_cursor_reset_count")
            .with_description(
                "Number of fetch_notes() requests where the client's cursor was \
                 above the legacy-cursor threshold and reset to 0 (pre-seq-migration \
                 clients)",
            )
            .build();

        let maintenance_cleanup_notes_count = meter
            .u64_counter("db_maintenance_cleanup_notes_count")
            .with_description("Total number of DB maintenance cleanup_old_notes() requests")
            .build();

        let maintenance_cleanup_notes_duration = meter
            .f64_histogram("db_maintenance_cleanup_notes_duration")
            .with_description("Duration of DB maintenance cleanup_old_notes() requests in seconds")
            .with_unit("s")
            .build();

        Self {
            store_note_count,
            store_note_duration,
            fetch_notes_count,
            fetch_notes_duration,
            fetch_notes_legacy_cursor_reset_count,
            maintenance_cleanup_notes_count,
            maintenance_cleanup_notes_duration,
        }
    }

    /// Measure a DB store-note request
    ///
    /// Increases the request counter and measures request duration.
    pub fn db_store_note(&self) -> RequestTimer<'_> {
        let operation = "db.store_note";
        let counter = &self.store_note_count;
        let histogram = &self.store_note_duration;

        request_count_measure(operation, counter, histogram)
    }

    /// Measure a DB fetch-notes request
    ///
    /// Increases the request counter and measures request duration.
    pub fn db_fetch_notes(&self) -> RequestTimer<'_> {
        let operation = "db.fetch_notes";
        let counter = &self.fetch_notes_count;
        let histogram = &self.fetch_notes_duration;

        request_count_measure(operation, counter, histogram)
    }

    /// Record a legacy-cursor reset (pre-seq-migration client).
    pub fn db_fetch_notes_legacy_cursor_reset(&self) {
        self.fetch_notes_legacy_cursor_reset_count.add(1, &[]);
    }

    /// Measure a DB maintenance cleanup-old-notes procedure
    ///
    /// Increases the request counter and measures request duration.
    pub fn db_maintenance_cleanup_notes(&self) -> RequestTimer<'_> {
        let operation = "db.maintenance.cleanup_old_notes";
        let counter = &self.maintenance_cleanup_notes_count;
        let histogram = &self.maintenance_cleanup_notes_duration;

        request_count_measure(operation, counter, histogram)
    }
}

/// Measure a request
///
/// Increases the request counter and measures request duration.
fn request_count_measure<'a>(
    operation: &str,
    counter: &Counter<u64>,
    histogram: &'a Histogram<f64>,
) -> RequestTimer<'a> {
    let start = std::time::Instant::now();

    // Increment request counter
    counter.add(1, &[KeyValue::new("operation", operation.to_string())]);

    RequestTimer {
        operation: operation.to_string(),
        start,
        histogram,
    }
}

impl Default for Metrics {
    fn default() -> Self {
        let meter = opentelemetry::global::meter("miden-note-transport-node");
        Self::new(&meter)
    }
}

/// Timer for measuring request duration
pub struct RequestTimer<'a> {
    operation: String,
    start: std::time::Instant,
    histogram: &'a Histogram<f64>,
}

impl RequestTimer<'_> {
    /// Finish the request and record the duration
    ///
    /// Consumes the timer so that a request is recorded exactly once. A timer that is dropped
    /// without reaching here is recorded by [`Drop`] instead, with `status = "error"`.
    pub fn finish(self, status: &str) {
        // Recording here and letting `Drop` run as well would enter every request twice.
        let this = ManuallyDrop::new(self);
        this.record(status);
    }

    /// Record the elapsed duration against this timer's operation and the given status.
    fn record(&self, status: &str) {
        let duration_s = self.start.elapsed().as_secs_f64();

        self.histogram.record(
            duration_s,
            &[
                KeyValue::new("operation", self.operation.clone()),
                KeyValue::new("status", status.to_string()),
            ],
        );
    }
}

impl Drop for RequestTimer<'_> {
    /// A timer that goes out of scope without [`RequestTimer::finish`] means the handler returned
    /// early, i.e. the request failed.
    fn drop(&mut self) {
        self.record("error");
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;

    /// Collect the `status` attribute of every data point recorded against `metric_name`.
    ///
    /// Each entry is one data point, so repeated statuses mean repeated recordings.
    fn recorded_statuses(exporter: &InMemoryMetricExporter, metric_name: &str) -> Vec<String> {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

        let mut statuses = Vec::new();

        for resource_metrics in exporter.get_finished_metrics().unwrap() {
            for scope in resource_metrics.scope_metrics() {
                for metric in scope.metrics().filter(|m| m.name() == metric_name) {
                    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data()
                    else {
                        panic!("{metric_name} is not an f64 histogram");
                    };

                    for point in histogram.data_points() {
                        for attribute in point.attributes() {
                            if attribute.key.as_str() == "status" {
                                statuses.push(attribute.value.to_string());
                            }
                        }
                    }
                }
            }
        }

        statuses.sort();
        statuses
    }

    /// Drive `record_request` against a private meter and return what reached the exporter.
    fn statuses_for(record_request: impl FnOnce(&MetricsGrpc)) -> Vec<String> {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();

        let metrics = MetricsGrpc::new(&provider.meter("test"));
        record_request(&metrics);

        provider.force_flush().unwrap();

        recorded_statuses(&exporter, "grpc_send_note_duration")
    }

    /// A finished request must produce exactly one duration sample. Previously `finish` took
    /// `&self` and `Drop` recorded unconditionally, so every success was counted twice — once as
    /// `ok` and once as `dropped` — inflating sample counts and firing the only error signal on
    /// the happy path.
    #[test]
    fn a_finished_request_is_recorded_once() {
        let statuses = statuses_for(|metrics| {
            metrics.grpc_send_note_request(64).finish("ok");
        });

        assert_eq!(statuses, ["ok"]);
    }

    /// A handler that returns early drops the timer without finishing it. That is a failed
    /// request and must be recorded as such, exactly once.
    #[test]
    fn an_abandoned_request_is_recorded_once_as_an_error() {
        let statuses = statuses_for(|metrics| {
            let _timer = metrics.grpc_send_note_request(64);
        });

        assert_eq!(statuses, ["error"]);
    }
}

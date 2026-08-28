use std::cell::Cell;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter, UpDownCounter};

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
    error_count: Counter<u64>,
    rejected_write_count: Counter<u64>,
    active_streams: UpDownCounter<i64>,
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
    retained_bytes: Gauge<u64>,
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

        let error_count = meter
            .u64_counter("grpc_error_count")
            .with_description("Total number of application RPC errors")
            .build();
        let rejected_write_count = meter
            .u64_counter("grpc_rejected_write_count")
            .with_description("Total number of note writes rejected by a resource limit")
            .build();
        let active_streams = meter
            .i64_up_down_counter("grpc_active_streams")
            .with_description("Current number of live StreamNotes RPCs")
            .build();

        Self {
            send_note_count,
            send_note_duration,
            send_note_note_size,
            fetch_notes_count,
            fetch_notes_duration,
            fetch_notes_replied_notes_number,
            fetch_notes_replied_notes_size,
            error_count,
            rejected_write_count,
            active_streams,
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

    /// Record one application RPC error.
    pub fn error(&self, operation: &'static str, status: tonic::Code) {
        self.error_count.add(
            1,
            &[
                KeyValue::new("operation", operation),
                KeyValue::new("status", status.to_string()),
            ],
        );
    }

    /// Record a write rejected by an application resource limit.
    pub fn rejected_write(&self, reason: &'static str) {
        self.rejected_write_count.add(1, &[KeyValue::new("reason", reason)]);
    }

    /// Increment the live stream gauge until the returned guard is dropped.
    pub fn stream_started(&self) -> ActiveStreamGuard {
        self.active_streams.add(1, &[]);
        ActiveStreamGuard { counter: self.active_streams.clone() }
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

        let retained_bytes = meter
            .u64_gauge("db_retained_bytes")
            .with_description("Current retained note header and detail bytes")
            .with_unit("B")
            .build();

        Self {
            store_note_count,
            store_note_duration,
            fetch_notes_count,
            fetch_notes_duration,
            retained_bytes,
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

    /// Record the retained payload byte count.
    pub fn record_retained_bytes(&self, bytes: u64) {
        self.retained_bytes.record(bytes, &[]);
    }
}

/// Decrements the active stream gauge when a streaming task ends.
pub struct ActiveStreamGuard {
    counter: UpDownCounter<i64>,
}

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        self.counter.add(-1, &[]);
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
        finished: Cell::new(false),
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
    finished: Cell<bool>,
}

impl RequestTimer<'_> {
    /// Finish the request and record the duration
    pub fn finish(&self, status: &str) {
        self.record(status);
        self.finished.set(true);
    }

    fn record(&self, status: &str) {
        let duration = self.start.elapsed();
        let duration_s = duration.as_secs_f64();

        // Record request duration
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
    fn drop(&mut self) {
        if !self.finished.get() {
            self.record("dropped");
        }
    }
}

use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::Utc;
use miden_note_transport_proto::FILE_DESCRIPTOR_SET;
use miden_note_transport_proto::miden_note_transport::v1::miden_note_transport_server::MidenNoteTransportServer;
use miden_note_transport_proto::miden_note_transport::v1::{
    FetchNotesRequest,
    FetchNotesResponse,
    SendNoteRequest,
    SendNoteResponse,
    StreamNotesRequest,
    TransportNote,
};
use miden_protocol::crypto::ies::SealedMessage;
use miden_protocol::utils::serde::Deserializable;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Semaphore, mpsc, watch};
use tonic::Status;
use tonic::codegen::tokio_stream::Stream;
use tonic::transport::server::{Connected, TcpIncoming};
use tonic_web::GrpcWebLayer;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower_http::cors::{Any, CorsLayer};

use crate::database::Database;
use crate::metrics::MetricsGrpc;

/// Upper bound on the number of tags a client may include in a single
/// `fetch_notes` request. Guards against two concerns:
///   - Server CPU: deduplicating `request_data.tags` via `BTreeSet` is `O(n log n)`; a client
///     sending millions of tags can burn a worker.
///   - `SQLite` `IN (...)`: the underlying driver caps bound variables at
///     `SQLITE_MAX_VARIABLE_NUMBER` (32766 on recent builds, lower on older); blow that and the
///     query errors. Well below the `SQLite` cap so we have headroom for future query-plan changes.
///
/// A realistic wallet tracks O(10) to O(100) tags; 128 is generous without
/// being an attack surface.
const MAX_TAGS_PER_FETCH_REQUEST: usize = 128;
const READINESS_TIMEOUT: Duration = Duration::from_secs(1);

/// Miden Note Transport gRPC server
pub struct GrpcServer {
    database: Arc<Database>,
    config: GrpcServerConfig,
    metrics: MetricsGrpc,
    stream_slots: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
}

/// [`GrpcServer`] configuration
#[derive(Clone, Debug)]
pub struct GrpcServerConfig {
    /// Address and port to bind.
    pub listen: SocketAddr,
    /// Maximum note size to be stored
    pub max_note_size: usize,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Connection timeout in seconds
    pub request_timeout: usize,
    /// Maximum bytes retained by storage.
    pub max_storage_bytes: u64,
    /// Maximum number of live streaming requests.
    pub max_streams: usize,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:57292".parse().expect("default listen address must be valid"),
            max_note_size: 512_000,
            max_connections: 4096,
            request_timeout: 4,
            max_storage_bytes: 1024 * 1024 * 1024,
            max_streams: 1024,
        }
    }
}

impl GrpcServer {
    /// gRPC server constructor
    pub fn new(database: Arc<Database>, config: GrpcServerConfig, metrics: MetricsGrpc) -> Self {
        let stream_slots = Arc::new(Semaphore::new(config.max_streams));
        let (shutdown, _) = watch::channel(false);
        Self {
            database,
            config,
            metrics,
            stream_slots,
            shutdown,
        }
    }

    /// Convert into a service
    pub fn into_service(self) -> MidenNoteTransportServer<Self> {
        MidenNoteTransportServer::new(self)
    }

    /// gRPC server running-task
    pub async fn serve(self) -> crate::Result<()> {
        let listen = self.config.listen;
        let incoming = TcpIncoming::bind(listen)
            .map_err(|e| crate::Error::Internal(format!("Failed to bind {listen}: {e}")))?;
        self.serve_with_incoming(incoming).await
    }

    async fn serve_with_incoming(self, incoming: TcpIncoming) -> crate::Result<()> {
        let (health_reporter, health_svc) = tonic_health::server::health_reporter();
        health_reporter
            .set_service_status("", tonic_health::ServingStatus::Serving)
            .await;
        set_api_health(&health_reporter, database_is_ready(&self.database).await).await;

        let database = self.database.clone();
        let mut readiness_shutdown = self.shutdown.subscribe();
        let readiness_reporter = health_reporter.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = readiness_shutdown.changed() => {
                        if result.is_err() || *readiness_shutdown.borrow() {
                            return;
                        }
                    },
                    () = tokio::time::sleep(Duration::from_secs(1)) => {
                        set_api_health(&readiness_reporter, database_is_ready(&database).await).await;
                    },
                }
            }
        });

        let reflection_svc = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
            .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
            .build_v1()
            .map_err(|e| {
                crate::Error::Internal(format!("Failed to build reflection service: {e}"))
            })?;

        let cors = CorsLayer::new().allow_origin(Any).allow_headers(Any).allow_methods(Any);
        let max_connections = u32::try_from(self.config.max_connections).map_err(|_| {
            crate::Error::Internal("max connections exceeds the HTTP/2 stream limit".to_string())
        })?;
        let request_timeout = self.config.request_timeout;
        let signal_shutdown = self.shutdown.clone();
        let serve_shutdown = self.shutdown.subscribe();
        let drain_shutdown = self.shutdown.subscribe();
        let (force_shutdown, _) = watch::channel(false);
        let incoming = ShutdownIncoming::new(incoming, force_shutdown.clone());
        let signal_task = tokio::spawn(shutdown_signal(signal_shutdown));
        let drain_task = tokio::spawn(async move {
            wait_for_shutdown(drain_shutdown).await;
            tokio::time::sleep(Duration::from_secs(request_timeout.saturating_add(1) as u64)).await;
            tracing::warn!("graceful shutdown deadline reached; closing remaining connections");
            force_shutdown.send_replace(true);
        });

        let server = tonic::transport::Server::builder()
            .accept_http1(true)
            .max_concurrent_streams(Some(max_connections))
            .http2_keepalive_interval(Some(Duration::from_secs(30)))
            .http2_keepalive_timeout(Some(Duration::from_secs(10)))
            .layer(cors)
            .layer(GrpcWebLayer::new())
            .layer(GlobalConcurrencyLimitLayer::new(self.config.max_connections))
            .layer(TimeoutLayer::new(Duration::from_secs(request_timeout as u64)))
            .add_service(health_svc)
            .add_service(reflection_svc)
            .add_service(self.into_service())
            .serve_with_incoming_shutdown(incoming, wait_for_shutdown(serve_shutdown));
        let result = server.await.map_err(|e| crate::Error::Internal(format!("Server error: {e}")));
        signal_task.abort();
        drain_task.abort();
        result
    }
}

async fn database_is_ready(database: &Database) -> bool {
    tokio::time::timeout(READINESS_TIMEOUT, database.is_ready())
        .await
        .unwrap_or(false)
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

struct ShutdownIncoming {
    inner: TcpIncoming,
    force_shutdown: watch::Sender<bool>,
}

impl ShutdownIncoming {
    fn new(inner: TcpIncoming, force_shutdown: watch::Sender<bool>) -> Self {
        Self { inner, force_shutdown }
    }
}

impl Stream for ShutdownIncoming {
    type Item = io::Result<ShutdownIo>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
            item.map(|result| {
                result.map(|inner| ShutdownIo::new(inner, self.force_shutdown.subscribe()))
            })
        })
    }
}

struct ShutdownIo {
    inner: tokio::net::TcpStream,
    force_shutdown: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl ShutdownIo {
    fn new(inner: tokio::net::TcpStream, force_shutdown: watch::Receiver<bool>) -> Self {
        Self {
            inner,
            force_shutdown: Box::pin(wait_for_shutdown(force_shutdown)),
        }
    }

    fn poll_force_shutdown(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if self.force_shutdown.as_mut().poll(cx).is_ready() {
            Err(io::Error::new(io::ErrorKind::ConnectionAborted, "server shutdown deadline"))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for ShutdownIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.poll_force_shutdown(cx)?;
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

impl AsyncWrite for ShutdownIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        self.poll_force_shutdown(cx)?;
        Pin::new(&mut self.inner).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.poll_force_shutdown(cx)?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Connected for ShutdownIo {
    type ConnectInfo = <tokio::net::TcpStream as Connected>::ConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.inner.connect_info()
    }
}

async fn set_api_health(reporter: &tonic_health::server::HealthReporter, ready: bool) {
    if ready {
        reporter.set_serving::<MidenNoteTransportServer<GrpcServer>>().await;
    } else {
        reporter.set_not_serving::<MidenNoteTransportServer<GrpcServer>>().await;
    }
}

#[tonic::async_trait]
impl miden_note_transport_proto::miden_note_transport::v1::miden_note_transport_server::MidenNoteTransport
    for GrpcServer
{
    #[tracing::instrument(skip(self, request), fields(
        operation = "grpc.send_note.request",
        note_size = tracing::field::Empty,
    ))]
    async fn send_note(
        &self,
        request: tonic::Request<SendNoteRequest>,
    ) -> Result<tonic::Response<SendNoteResponse>, tonic::Status> {
        let request_data = request.into_inner();
        let pnote = request_data.note.ok_or_else(|| {
            self.metrics.error("send_note", tonic::Code::InvalidArgument);
            Status::invalid_argument("Missing note")
        })?;

        // `header` + `details` are the stored payload; the cap, the metric, and
        // the span field all use the same number so accept and reject report
        // the same size.
        let note_size = pnote.header.len() + pnote.details.len();
        let span = tracing::Span::current();
        span.record("note_size", note_size);

        let timer = self.metrics.grpc_send_note_request(note_size as u64);

        // Validate note size
        if note_size > self.config.max_note_size {
            tracing::warn!(reason = "note_too_large", size = note_size, max = self.config.max_note_size, "send_note rejected");
            self.metrics.rejected_write("envelope_size");
            self.metrics.error("send_note", tonic::Code::ResourceExhausted);
            timer.finish("resource_exhausted");
            return Err(Status::resource_exhausted(format!("Note too large ({note_size})")));
        }

        // Convert protobuf request to internal types
        let header = miden_protocol::note::NoteHeader::read_from_bytes(&pnote.header)
            .map_err(|e| {
                tracing::warn!(reason = "invalid_header", "send_note rejected");
                self.metrics.error("send_note", tonic::Code::InvalidArgument);
                Status::invalid_argument(format!("Invalid header: {e:?}"))
            })?;
        if SealedMessage::read_from_bytes(&pnote.details).is_err() {
            self.metrics.error("send_note", tonic::Code::InvalidArgument);
            return Err(Status::invalid_argument(
                "note details are not a valid sealed message",
            ));
        }

        tracing::debug!(
            note_id = %header.id(),
            tag = header.metadata().tag().as_u32(),
            has_after_block_num = pnote.after_block_num.is_some(),
            "send_note accepted"
        );

        // Create note for database
        let note_for_db = crate::types::StoredNote {
            header,
            details: pnote.details,
            created_at: Utc::now(),
            // Ignored on INSERT: the DB assigns seq via AUTOINCREMENT.
            seq: 0,
            after_block_num: pnote.after_block_num,
        };

        self.database
            .store_note(&note_for_db, self.config.max_storage_bytes)
            .await
            .map_err(|error| match error {
                crate::database::DatabaseError::Capacity(message) => {
                    self.metrics.rejected_write("storage_capacity");
                    self.metrics.error("send_note", tonic::Code::ResourceExhausted);
                    tonic::Status::resource_exhausted(message)
                },
                error => {
                    self.metrics.error("send_note", tonic::Code::Unavailable);
                    tonic::Status::unavailable(format!("Failed to store note: {error:?}"))
                },
            })?;

        timer.finish("ok");

        Ok(tonic::Response::new(SendNoteResponse {}))
    }

    #[tracing::instrument(skip(self, request), fields(
        operation = "grpc.fetch_notes.request",
        tag_count = tracing::field::Empty,
        cursor = tracing::field::Empty,
        notes_returned = tracing::field::Empty,
        response_cursor = tracing::field::Empty,
    ))]
    async fn fetch_notes(
        &self,
        request: tonic::Request<FetchNotesRequest>,
    ) -> Result<tonic::Response<FetchNotesResponse>, tonic::Status> {
        let timer = self.metrics.grpc_fetch_notes_request();

        let request_data = request.into_inner();

        // Reject requests with too many tags BEFORE any allocation /
        // deduplication work. A client sending `[0u32; 1_000_000]` would
        // otherwise force an O(n log n) BTreeSet build and then either blow
        // through `SQLITE_MAX_VARIABLE_NUMBER` or return a pathologically
        // expensive query plan.
        if request_data.tags.len() > MAX_TAGS_PER_FETCH_REQUEST {
            tracing::warn!(
                reason = "too_many_tags",
                tag_count = request_data.tags.len(),
                max = MAX_TAGS_PER_FETCH_REQUEST,
                "fetch_notes rejected"
            );
            self.metrics.error("fetch_notes", tonic::Code::InvalidArgument);
            timer.finish("invalid_argument");
            return Err(Status::invalid_argument(format!(
                "Too many tags in fetch_notes request: {} (max {})",
                request_data.tags.len(),
                MAX_TAGS_PER_FETCH_REQUEST
            )));
        }

        // Deduplicate incoming tags — the DB query is more efficient without repeats
        // and the previous per-tag loop happened to dedupe via BTreeSet.
        let tag_set: BTreeSet<_> = request_data.tags.into_iter().collect();
        let tags: Vec<crate::types::NoteTag> = tag_set.into_iter().map(Into::into).collect();
        let cursor = request_data.cursor;

        let span = tracing::Span::current();
        span.record("tag_count", tags.len());
        span.record("cursor", cursor);

        // Single-snapshot fetch across ALL tags. Running per-tag queries back
        // to back exposed a race where a concurrent INSERT could land between
        // two per-tag queries and get leapfrogged when rcursor advanced past
        // its seq on the next fetch. A single `tag IN (…)` query reads all
        // matching rows in one consistent snapshot.
        let page = self
            .database
            .fetch_notes_by_tags(&tags, cursor)
            .await
            .map_err(|e| {
                self.metrics.error("fetch_notes", tonic::Code::Unavailable);
                tonic::Status::unavailable(format!("Failed to fetch notes: {e:?}"))
            })?;

        let mut rcursor = cursor;
        for stored_note in &page.notes {
            let seq_cursor: u64 = stored_note
                .seq
                .try_into()
                .map_err(|_| {
                    self.metrics.error("fetch_notes", tonic::Code::Internal);
                    tonic::Status::internal("Negative seq in stored note")
                })?;
            rcursor = rcursor.max(seq_cursor);
        }

        let proto_notes: Vec<_> = page.notes.into_iter().map(TransportNote::from).collect();

        span.record("notes_returned", proto_notes.len());
        span.record("response_cursor", rcursor);

        timer.finish("ok");

        let proto_notes_size = proto_notes.iter().map(|pnote| (pnote.header.len() + pnote.details.len()) as u64).sum();
        self.metrics.grpc_fetch_notes_response(
            proto_notes.len() as u64,
            proto_notes_size,
        );

        Ok(tonic::Response::new(FetchNotesResponse {
            notes: proto_notes,
            cursor: rcursor,
            has_more: page.has_more,
        }))
    }

    type StreamNotesStream = tonic::codegen::tokio_stream::wrappers::ReceiverStream<
        Result<miden_note_transport_proto::miden_note_transport::v1::StreamNotesUpdate, Status>,
    >;
    #[tracing::instrument(skip(self, request), fields(
        operation = "grpc.stream_notes.request",
        subscription_id = tracing::field::Empty,
    ))]
    async fn stream_notes(
        &self,
        request: tonic::Request<StreamNotesRequest>,
    ) -> Result<tonic::Response<Self::StreamNotesStream>, tonic::Status> {
        let stream_slot = self.stream_slots.clone().try_acquire_owned().map_err(|_| {
            self.metrics.error("stream_notes", tonic::Code::ResourceExhausted);
            Status::resource_exhausted("too many live streams")
        })?;
        let request_data = request.into_inner();
        let tag = request_data.tag.into();
        let cursor = request_data.cursor;
        let database = self.database.clone();
        let changes = database.subscribe();
        let shutdown = self.shutdown.subscribe();
        let metrics = self.metrics.clone();
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let _stream_slot = stream_slot;
            let _active_stream = metrics.stream_started();
            stream_notes(database, tag, cursor, changes, shutdown, tx, metrics).await;
        });

        Ok(tonic::Response::new(
            tonic::codegen::tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }
}

async fn stream_notes(
    database: Arc<Database>,
    tag: crate::types::NoteTag,
    mut cursor: u64,
    mut changes: tokio::sync::watch::Receiver<crate::database::DatabaseWatch>,
    mut shutdown: watch::Receiver<bool>,
    tx: mpsc::Sender<
        Result<miden_note_transport_proto::miden_note_transport::v1::StreamNotesUpdate, Status>,
    >,
    metrics: MetricsGrpc,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if !changes.borrow().is_ready() {
            metrics.error("stream_notes", tonic::Code::Unavailable);
            send_stream_error(
                &tx,
                &mut shutdown,
                Status::unavailable("note storage change notifications are unavailable"),
            )
            .await;
            return;
        }
        let fetched = tokio::select! {
            result = database.fetch_notes(tag, cursor) => result,
            result = shutdown.changed() => {
                let _ = result;
                return;
            },
        };
        match fetched {
            Ok(page) if !page.notes.is_empty() => {
                let Ok(next_cursor) = crate::database::advance_cursor(&page.notes, cursor) else {
                    metrics.error("stream_notes", tonic::Code::Internal);
                    send_stream_error(&tx, &mut shutdown, Status::internal("invalid note cursor"))
                        .await;
                    return;
                };
                cursor = next_cursor;
                let update =
                    miden_note_transport_proto::miden_note_transport::v1::StreamNotesUpdate {
                        notes: page.notes.into_iter().map(TransportNote::from).collect(),
                        cursor,
                    };
                tokio::select! {
                    result = tx.send(Ok(update)) => {
                        if result.is_err() {
                            return;
                        }
                    },
                    result = shutdown.changed() => {
                        let _ = result;
                        return;
                    },
                };
                continue;
            },
            Ok(_) => {},
            Err(error) => {
                metrics.error("stream_notes", tonic::Code::Unavailable);
                send_stream_error(&tx, &mut shutdown, Status::unavailable(error.to_string())).await;
                return;
            },
        }

        tokio::select! {
            result = changes.changed() => {
                if result.is_err() {
                    return;
                }
                changes.borrow_and_update();
            },
            () = tx.closed() => return,
            result = shutdown.changed() => {
                let _ = result;
                return;
            },
        }
    }
}

async fn send_stream_error(
    tx: &mpsc::Sender<
        Result<miden_note_transport_proto::miden_note_transport::v1::StreamNotesUpdate, Status>,
    >,
    shutdown: &mut watch::Receiver<bool>,
    status: Status,
) {
    tokio::select! {
        _ = tx.send(Err(status)) => {},
        _ = shutdown.changed() => {},
        () = tx.closed() => {},
    }
}

#[cfg(unix)]
async fn shutdown_signal(shutdown: watch::Sender<bool>) {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
                shutdown.send_replace(true);
                return;
            },
        };

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                tracing::error!(%error, "failed to install Ctrl-C handler");
            }
        },
        _ = terminate.recv() => {},
    }
    shutdown.send_replace(true);
}

#[cfg(not(unix))]
async fn shutdown_signal(shutdown: watch::Sender<bool>) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
    shutdown.send_replace(true);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use miden_note_transport_proto::miden_note_transport::v1::miden_note_transport_server::MidenNoteTransport;
    use miden_note_transport_proto::miden_note_transport::v1::{
        FetchNotesRequest,
        StreamNotesRequest,
    };
    use tonic::codegen::tokio_stream::StreamExt;

    use super::*;
    use crate::database::Database;
    use crate::metrics::Metrics;
    use crate::test_utils::{TAG_LOCAL_ANY, test_note_header};
    use crate::types::StoredNote;

    async fn test_server_with_database() -> (GrpcServer, Arc<Database>) {
        let metrics = Metrics::default();
        let db = Arc::new(Database::connect_for_test(metrics.db.clone()).await.unwrap());
        (GrpcServer::new(db.clone(), GrpcServerConfig::default(), metrics.grpc), db)
    }

    async fn test_server() -> GrpcServer {
        test_server_with_database().await.0
    }

    /// A client sending more tags than `MAX_TAGS_PER_FETCH_REQUEST` is rejected
    /// with `InvalidArgument` BEFORE any `BTreeSet` or DB work. Guards against
    /// both the O(n log n) dedup cost and the `SQLITE_MAX_VARIABLE_NUMBER`
    /// ceiling.
    #[tokio::test]
    async fn test_fetch_notes_rejects_too_many_tags() {
        let server = test_server().await;

        let tags = vec![0u32; MAX_TAGS_PER_FETCH_REQUEST + 1];
        let request = tonic::Request::new(FetchNotesRequest { tags, cursor: 0 });
        let result = server.fetch_notes(request).await;

        let status = result.expect_err("expected InvalidArgument");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(
            status.message().contains("Too many tags"),
            "unexpected error message: {}",
            status.message()
        );
    }

    /// A client sending exactly `MAX_TAGS_PER_FETCH_REQUEST` tags is accepted.
    /// (Using the same tag value many times is fine — the handler dedups via
    /// `BTreeSet` before issuing the query.)
    #[tokio::test]
    async fn test_fetch_notes_accepts_max_tags_at_limit() {
        let server = test_server().await;

        let tags = vec![0u32; MAX_TAGS_PER_FETCH_REQUEST];
        let request = tonic::Request::new(FetchNotesRequest { tags, cursor: 0 });
        let result = server.fetch_notes(request).await;

        let response = result.expect("request at the cap must succeed").into_inner();
        assert_eq!(response.notes.len(), 0, "DB is empty, no notes returned");
        assert_eq!(response.cursor, 0);
    }

    #[tokio::test]
    async fn stream_waits_for_a_post_commit_notification() {
        let (server, database) = test_server_with_database().await;
        let response = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor: 0 }))
            .await
            .unwrap();
        let mut stream = response.into_inner();

        assert!(
            tokio::time::timeout(Duration::from_millis(25), stream.next()).await.is_err(),
            "an idle stream must wait instead of producing empty pages",
        );

        database
            .store_note(
                &StoredNote {
                    header: test_note_header(),
                    details: vec![1, 2, 3],
                    created_at: Utc::now(),
                    seq: 0,
                    after_block_num: None,
                },
                u64::MAX,
            )
            .await
            .unwrap();
        let update = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(update.notes.len(), 1);
        assert!(update.cursor > 0);
    }

    #[tokio::test]
    async fn stream_limit_is_held_for_the_stream_lifetime() {
        let metrics = Metrics::default();
        let database = Arc::new(Database::connect_for_test(metrics.db.clone()).await.unwrap());
        let config = GrpcServerConfig {
            max_streams: 1,
            ..GrpcServerConfig::default()
        };
        let server = GrpcServer::new(database, config, metrics.grpc);

        let first = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor: 0 }))
            .await
            .unwrap();
        let second = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor: 0 }))
            .await
            .expect_err("the second live stream must exceed the configured limit");

        assert_eq!(second.code(), tonic::Code::ResourceExhausted);
        drop(first);
    }

    #[tokio::test]
    async fn shutdown_ends_an_idle_stream() {
        let server = test_server().await;
        let shutdown = server.shutdown.clone();
        let response = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor: 0 }))
            .await
            .unwrap();
        let mut stream = response.into_inner();

        shutdown.send_replace(true);

        let ended = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("stream did not end after shutdown");
        assert!(ended.is_none());
    }

    #[tokio::test]
    async fn shutdown_interrupts_a_blocked_stream_error() {
        let (tx, _rx) = mpsc::channel(1);
        tx.send(Ok(miden_note_transport_proto::miden_note_transport::v1::StreamNotesUpdate {
            notes: Vec::new(),
            cursor: 0,
        }))
        .await
        .unwrap();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let blocked = tokio::spawn(async move {
            send_stream_error(&tx, &mut shutdown_rx, Status::unavailable("storage unavailable"))
                .await;
        });

        shutdown_tx.send_replace(true);

        tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .expect("blocked error delivery ignored shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn shutdown_deadline_closes_a_health_watch() {
        let metrics = Metrics::default();
        let database = Arc::new(Database::connect_for_test(metrics.db.clone()).await.unwrap());
        let config = GrpcServerConfig {
            request_timeout: 1,
            ..GrpcServerConfig::default()
        };
        let server = GrpcServer::new(database, config, metrics.grpc);
        let shutdown = server.shutdown.clone();
        let incoming = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = incoming.local_addr().unwrap();
        let mut server_task = tokio::spawn(server.serve_with_incoming(incoming));
        let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        let mut client = tonic_health::pb::health_client::HealthClient::new(channel);
        let mut health_watch = client
            .watch(tonic::Request::new(tonic_health::pb::HealthCheckRequest {
                service: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        health_watch.message().await.unwrap().unwrap();

        shutdown.send_replace(true);

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut server_task)
                .await
                .is_err(),
            "the health watch must hold graceful shutdown open until the deadline",
        );
        tokio::time::timeout(Duration::from_secs(3), server_task)
            .await
            .expect("server exceeded its graceful shutdown deadline")
            .unwrap()
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(1), health_watch.message())
            .await
            .expect("health connection remained open after the deadline");
        assert!(closed.is_err() || closed.unwrap().is_none());
    }

    #[tokio::test]
    async fn stream_resumes_after_the_requested_cursor() {
        let (server, database) = test_server_with_database().await;
        for details in [vec![1], vec![2]] {
            database
                .store_note(
                    &StoredNote {
                        header: test_note_header(),
                        details,
                        created_at: Utc::now(),
                        seq: 0,
                        after_block_num: None,
                    },
                    u64::MAX,
                )
                .await
                .unwrap();
        }
        let stored = database.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        let cursor = u64::try_from(stored.notes[0].seq).unwrap();
        let response = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor }))
            .await
            .unwrap();
        let mut stream = response.into_inner();

        let update = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(update.notes.len(), 1);
        assert_eq!(update.notes[0].details, stored.notes[1].details);
        assert_eq!(update.cursor, u64::try_from(stored.notes[1].seq).unwrap());
    }

    #[tokio::test]
    async fn stream_drains_more_than_one_page_in_order() {
        let (server, database) = test_server_with_database().await;
        let mut expected = Vec::new();
        for value in 0..=crate::database::FETCH_NOTES_MAX_ROWS {
            let details = value.to_le_bytes().to_vec();
            database
                .store_note(
                    &StoredNote {
                        header: test_note_header(),
                        details: details.clone(),
                        created_at: Utc::now(),
                        seq: 0,
                        after_block_num: None,
                    },
                    u64::MAX,
                )
                .await
                .unwrap();
            expected.push(details);
        }

        let response = server
            .stream_notes(tonic::Request::new(StreamNotesRequest { tag: TAG_LOCAL_ANY, cursor: 0 }))
            .await
            .unwrap();
        let mut stream = response.into_inner();
        let first = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert_eq!(first.notes.len(), crate::database::FETCH_NOTES_MAX_ROWS as usize);
        assert_eq!(second.notes.len(), 1);
        assert!(first.cursor < second.cursor);
        let actual: Vec<_> =
            first.notes.into_iter().chain(second.notes).map(|note| note.details).collect();
        assert_eq!(actual, expected);
    }
}

mod streaming;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use miden_note_transport_proto::miden_note_transport::miden_note_transport_server::MidenNoteTransportServer;
use miden_note_transport_proto::miden_note_transport::{
    FetchNotesRequest,
    FetchNotesResponse,
    SendNoteRequest,
    SendNoteResponse,
    StatsResponse,
    StreamNotesRequest,
    TransportNote,
};
use miden_protocol::utils::serde::Deserializable;
use rand::Rng;
use tokio::sync::mpsc;
use tonic::Status;
use tonic_web::GrpcWebLayer;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower_http::cors::{Any, CorsLayer};

use self::streaming::{NoteStreamer, StreamerMessage, Sub, Subface};
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

/// Miden Note Transport gRPC server
pub struct GrpcServer {
    database: Arc<Database>,
    config: GrpcServerConfig,
    streamer: StreamerCtx,
    metrics: MetricsGrpc,
}

/// [`GrpcServer`] configuration
#[derive(Clone, Debug)]
pub struct GrpcServerConfig {
    /// Server host
    pub host: String,
    /// Server port
    pub port: u16,
    /// Maximum note size to be stored
    pub max_note_size: usize,
    /// Maximum number of concurrent connections
    pub max_connections: usize,
    /// Connection timeout in seconds
    pub request_timeout: usize,
}

/// Streaming task interface context
pub(super) struct StreamerCtx {
    tx: mpsc::Sender<StreamerMessage>,
    handle: tokio::task::JoinHandle<()>,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 57292,
            max_note_size: 512_000,
            max_connections: 4096,
            request_timeout: 4,
        }
    }
}

impl GrpcServer {
    /// gRPC server constructor
    pub fn new(database: Arc<Database>, config: GrpcServerConfig, metrics: MetricsGrpc) -> Self {
        let streamer = StreamerCtx::spawn(database.clone());
        Self { database, config, streamer, metrics }
    }

    /// Convert into a service
    pub fn into_service(self) -> MidenNoteTransportServer<Self> {
        MidenNoteTransportServer::new(self)
    }

    /// gRPC server running-task
    pub async fn serve(self) -> crate::Result<()> {
        let (health_reporter, health_svc) = tonic_health::server::health_reporter();
        health_reporter.set_serving::<MidenNoteTransportServer<Self>>().await;

        let addr = format!("{}:{}", self.config.host, self.config.port)
            .parse::<SocketAddr>()
            .map_err(|e| crate::Error::Internal(format!("Invalid address: {e}")))?;

        let cors = CorsLayer::new().allow_origin(Any).allow_headers(Any).allow_methods(Any);

        tonic::transport::Server::builder()
            .accept_http1(true)
            .layer(cors)
            .layer(GrpcWebLayer::new())
            .layer(GlobalConcurrencyLimitLayer::new(self.config.max_connections))
            .layer(TimeoutLayer::new(Duration::from_secs(self.config.request_timeout as u64)))
            .add_service(health_svc)
            .add_service(self.into_service())
            .serve(addr)
            .await
            .map_err(|e| crate::Error::Internal(format!("Server error: {e}")))
    }
}

impl StreamerCtx {
    /// Spawn a [`NoteStreamer`] task
    ///
    /// Returns related context composed of the handle and `mpsc::Sender` `tx` for control messages.
    pub(super) fn spawn(database: Arc<Database>) -> Self {
        let (tx, rx) = mpsc::channel(128);
        let handle = tokio::spawn(NoteStreamer::new(database, rx).stream());
        Self { tx, handle }
    }
}

#[tonic::async_trait]
impl miden_note_transport_proto::miden_note_transport::miden_note_transport_server::MidenNoteTransport
    for GrpcServer
{
    #[tracing::instrument(skip(self), fields(operation = "grpc.send_note.request"))]
    async fn send_note(
        &self,
        request: tonic::Request<SendNoteRequest>,
    ) -> Result<tonic::Response<SendNoteResponse>, tonic::Status> {
        let request_data = request.into_inner();
        let pnote = request_data.note.ok_or_else(|| Status::invalid_argument("Missing note"))?;

        let timer = self.metrics.grpc_send_note_request((pnote.header.len() + pnote.details.len()) as u64);

        // Validate note size (details + optional metadata)
        let payload_size = pnote.details.len() + pnote.note_metadata.as_ref().map_or(0, Vec::len);
        if payload_size > self.config.max_note_size {
            return Err(Status::resource_exhausted(format!("Note too large ({payload_size})")));
        }

        // Convert protobuf request to internal types
        let header = miden_protocol::note::NoteHeader::read_from_bytes(&pnote.header)
            .map_err(|e| Status::invalid_argument(format!("Invalid header: {e:?}")))?;

        // Create note for database
        let note_for_db = crate::types::StoredNote {
            header,
            details: pnote.details,
            created_at: Utc::now(),
            // Ignored on INSERT: the DB assigns seq via AUTOINCREMENT.
            seq: 0,
            commitment_block_num: pnote.commitment_block_num,
            note_metadata: pnote.note_metadata,
        };

        self.database
            .store_note(&note_for_db)
            .await.map_err(|e| tonic::Status::internal(format!("Failed to store note: {e:?}")))?;

        timer.finish("ok");

        Ok(tonic::Response::new(SendNoteResponse {}))
    }

    #[tracing::instrument(skip(self), fields(operation = "grpc.fetch_notes.request"))]
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

        // Single-snapshot fetch across ALL tags. Running per-tag queries back
        // to back exposed a race where a concurrent INSERT could land between
        // two per-tag queries and get leapfrogged when rcursor advanced past
        // its seq on the next fetch. A single `tag IN (…)` query reads all
        // matching rows in one consistent snapshot.
        let stored_notes = self
            .database
            .fetch_notes_by_tags(&tags, cursor)
            .await
            .map_err(|e| tonic::Status::internal(format!("Failed to fetch notes: {e:?}")))?;

        let mut rcursor = cursor;
        for stored_note in &stored_notes {
            let seq_cursor: u64 = stored_note
                .seq
                .try_into()
                .map_err(|_| tonic::Status::internal("Negative seq in stored note"))?;
            rcursor = rcursor.max(seq_cursor);
        }

        let proto_notes: Vec<_> = stored_notes.into_iter().map(TransportNote::from).collect();

        timer.finish("ok");

        let proto_notes_size = proto_notes.iter().map(|pnote| (pnote.header.len() + pnote.details.len()) as u64).sum();
        self.metrics.grpc_fetch_notes_response(
            proto_notes.len() as u64,
            proto_notes_size,
        );

        Ok(tonic::Response::new(FetchNotesResponse { notes: proto_notes, cursor: rcursor }))
    }

    type StreamNotesStream = Sub;
    #[tracing::instrument(skip(self), fields(operation = "grpc.stream_notes.request"))]
    async fn stream_notes(
        &self,
        request: tonic::Request<StreamNotesRequest>,
    ) -> Result<tonic::Response<Self::StreamNotesStream>, tonic::Status> {
        let request_data = request.into_inner();
        let tag = request_data.tag.into();
        let id = rand::rng().random();
        let (sub_tx, sub_rx) = mpsc::channel(32);
        let sub = Sub::new(id, tag, sub_rx, self.streamer.tx.clone());
        let subf = Subface::new(id, tag, sub_tx);
        self.streamer.tx.try_send(StreamerMessage::AddSub(subf))
                    .map_err(|e| tonic::Status::internal(format!("Failed sending internal streamer message: {e}")))?;

        Ok(tonic::Response::new(sub))
    }

    #[tracing::instrument(skip(self), fields(operation = "grpc.stats.request"))]
    async fn stats(
        &self,
        _request: tonic::Request<()>,
    ) -> Result<tonic::Response<StatsResponse>, tonic::Status> {
        let (total_notes, total_tags) = self
            .database
            .get_stats()
            .await.map_err(|e| tonic::Status::internal(format!("Failed to get stats: {e:?}")))?;

        let response = StatsResponse {
            total_notes,
            total_tags,
            notes_per_tag: Vec::new(), // TODO: Implement notes_per_tag
        };

        Ok(tonic::Response::new(response))
    }
}

impl Drop for StreamerCtx {
    fn drop(&mut self) {
        if let Err(e) = self.tx.try_send(StreamerMessage::Shutdown) {
            tracing::error!("Streamer shutdown message sending failure: {e}");
            self.handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use miden_note_transport_proto::miden_note_transport::FetchNotesRequest;
    use miden_note_transport_proto::miden_note_transport::miden_note_transport_server::MidenNoteTransport;

    use super::*;
    use crate::database::{Database, DatabaseConfig};
    use crate::metrics::Metrics;

    async fn test_server() -> GrpcServer {
        let metrics = Metrics::default();
        let db = Arc::new(
            Database::connect(DatabaseConfig::default(), metrics.db.clone()).await.unwrap(),
        );
        GrpcServer::new(db, GrpcServerConfig::default(), metrics.grpc)
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
}

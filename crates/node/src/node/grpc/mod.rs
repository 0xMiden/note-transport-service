mod rate_limit;
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
use miden_objects::utils::Deserializable;
use rand::Rng;
use tokio::sync::mpsc;
use tonic::Status;
use tonic_web::GrpcWebLayer;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower_http::cors::{Any, CorsLayer};

pub use self::rate_limit::RateLimitConfig;
use self::streaming::{NoteStreamer, StreamerMessage, Sub, Subface};
use crate::database::Database;
use crate::metrics::MetricsGrpc;

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
    /// Rate limiting configuration
    pub rate_limit: RateLimitConfig,
    /// TCP keepalive interval in seconds (None to disable)
    pub tcp_keepalive_secs: Option<u64>,
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
            rate_limit: RateLimitConfig::default(),
            tcp_keepalive_secs: Some(60),
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
        let rate_limit_layer = rate_limit::RateLimitLayer::new(&self.config.rate_limit);

        let mut builder = tonic::transport::Server::builder()
            .accept_http1(true)
            // TCP settings
            .tcp_nodelay(true);

        // TCP keepalive
        if let Some(keepalive_secs) = self.config.tcp_keepalive_secs {
            builder = builder.tcp_keepalive(Some(Duration::from_secs(keepalive_secs)));
        }

        builder
            .layer(cors)
            .layer(GrpcWebLayer::new())
            .layer(rate_limit_layer)
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

        // Validate note size
        if pnote.details.len() > self.config.max_note_size {
            return Err(Status::resource_exhausted(format!("Note too large ({})", pnote.details.len())));
        }

        // Convert protobuf request to internal types
        let header = miden_objects::note::NoteHeader::read_from_bytes(&pnote.header)
            .map_err(|e| Status::invalid_argument(format!("Invalid header: {e:?}")))?;

        // Create note for database
        let note_for_db = crate::types::StoredNote {
            header,
            details: pnote.details,
            created_at: Utc::now(),
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
        let tags = request_data.tags.into_iter().collect::<BTreeSet<_>>();
        let cursor = request_data.cursor;

        let mut rcursor = cursor;
        let mut proto_notes = vec![];
        for tag in tags {
            let stored_notes = self
                .database
                .fetch_notes(tag.into(), cursor)
                .await.map_err(|e| tonic::Status::internal(format!("Failed to fetch notes: {e:?}")))?;

            for stored_note in &stored_notes {
                let ts_cursor: u64 = stored_note
                    .created_at
                    .timestamp_micros()
                    .try_into()
                    .map_err(|_| tonic::Status::internal("Timestamp too large for cursor"))?;
                rcursor = rcursor.max(ts_cursor);
            }

            proto_notes.extend(stored_notes.into_iter().map(TransportNote::from));
        }

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

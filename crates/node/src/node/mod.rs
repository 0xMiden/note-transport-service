use std::sync::Arc;

use tracing::info;

use self::grpc::{GrpcServer, GrpcServerConfig};
use crate::Result;
use crate::database::{Database, DatabaseConfig};
use crate::metrics::Metrics;

/// gRPC server
pub mod grpc;

/// Miden Note Transport Node
pub struct Node {
    /// Serve client requests
    grpc: GrpcServer,
    /// Metrics
    _metrics: Metrics,

    // To be used in other services, .e.g. P2P
    _database: Arc<Database>,
}

/// Node configuration
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// gRPC server configuration
    pub grpc: GrpcServerConfig,
    /// Database configuration
    pub database: DatabaseConfig,
}

impl Node {
    /// Node constructor
    pub async fn init(config: NodeConfig) -> Result<Self> {
        if config.grpc.max_note_size > crate::database::FETCH_NOTES_MAX_BYTES {
            return Err(crate::Error::Internal(format!(
                "max note size cannot exceed {} bytes",
                crate::database::FETCH_NOTES_MAX_BYTES
            )));
        }
        if config.grpc.max_storage_bytes < config.grpc.max_note_size as u64 {
            return Err(crate::Error::Internal(
                "max storage bytes must be at least the maximum note size".to_string(),
            ));
        }
        if config.grpc.max_requests == 0 {
            return Err(crate::Error::Internal("max requests must be nonzero".to_string()));
        }
        if config.grpc.max_streams == 0 {
            return Err(crate::Error::Internal("max streams must be nonzero".to_string()));
        }
        if config.grpc.request_timeout == 0 {
            return Err(crate::Error::Internal("request timeout must be nonzero".to_string()));
        }
        let metrics = Metrics::default();
        let database =
            Arc::new(Database::connect(config.database.clone(), metrics.db.clone()).await?);

        let grpc = GrpcServer::new(database.clone(), config.grpc, metrics.grpc.clone());
        Ok(Self {
            grpc,
            _metrics: metrics,
            _database: database,
        })
    }

    /// Node running-task
    pub async fn entrypoint(self) -> Result<()> {
        info!("Starting Miden Transport Node");
        self.grpc.serve().await
    }
}

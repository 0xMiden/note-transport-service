use std::sync::Arc;

use tracing::{error, info};

use self::grpc::{GrpcServer, GrpcServerConfig};
use crate::Result;
use crate::database::{Database, DatabaseConfig, DatabaseMaintenance};
use crate::metrics::Metrics;

/// gRPC server
pub mod grpc;

/// Miden Note Transport Node
pub struct Node {
    /// Serve client requests
    grpc: GrpcServer,
    /// Database maintenance
    maintenance: DatabaseMaintenance,
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
        let metrics = Metrics::default();
        let database =
            Arc::new(Database::connect(config.database.clone(), metrics.db.clone()).await?);

        let grpc = GrpcServer::new(database.clone(), config.grpc, metrics.grpc.clone());
        let maintenance =
            DatabaseMaintenance::new(database.clone(), config.database, metrics.db.clone());

        Ok(Self {
            grpc,
            maintenance,
            _metrics: metrics,
            _database: database,
        })
    }

    /// Node running-task
    pub async fn entrypoint(self) {
        info!("Starting Miden Transport Node");
        tokio::spawn(self.maintenance.entrypoint());

        if let Err(e) = self.grpc.serve().await {
            error!("Server error: {e}");
        }
    }
}

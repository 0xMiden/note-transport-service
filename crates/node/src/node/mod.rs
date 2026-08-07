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
#[derive(Debug, Default, Clone)]
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
    ///
    /// Returns the error that brought the server down, so that the process can exit non-zero and
    /// supervisors configured to restart on failure actually restart it.
    pub async fn entrypoint(self) -> Result<()> {
        info!("Starting Miden Transport Node");
        tokio::spawn(self.maintenance.entrypoint());

        self.grpc.serve().await.inspect_err(|e| error!("Server error: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// A fatal server error must reach the caller rather than being logged and swallowed,
    /// otherwise the process exits 0 and `restart: on-failure` never fires.
    #[tokio::test]
    async fn entrypoint_returns_the_error_that_brought_the_server_down() {
        // Hold the port so that the node's own bind is guaranteed to fail.
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();

        let config = NodeConfig {
            grpc: GrpcServerConfig {
                host: "127.0.0.1".into(),
                port,
                ..Default::default()
            },
            database: DatabaseConfig::default(),
        };

        let node = Node::init(config).await.unwrap();

        let err = node.entrypoint().await.unwrap_err();

        assert!(err.to_string().contains("Server error"), "unexpected error: {err}");
    }
}

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
    /// Serves until `shutdown` resolves, then drains in-flight requests, stops the note streamer
    /// and the maintenance loop, and returns.
    ///
    /// Returns the error that brought the server down, so that the process can exit non-zero and
    /// supervisors configured to restart on failure actually restart it.
    pub async fn entrypoint(
        self,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        info!("Starting Miden Transport Node");
        let maintenance = tokio::spawn(self.maintenance.entrypoint());

        let result = self
            .grpc
            .serve_with_shutdown(shutdown)
            .await
            .inspect_err(|e| error!("Server error: {e}"));

        // The maintenance loop spends almost all of its time sleeping between cleanup passes and
        // holds no client-visible state, so there is nothing to drain — aborting it mid-sleep is
        // the intended stop. A cleanup pass interrupted mid-transaction rolls back, and the next
        // start picks the same expired rows up again.
        maintenance.abort();
        info!("Maintenance loop stopped");

        result
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    /// Bind to port 0 to reserve a free port, then release it for the node to claim.
    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn test_config(port: u16) -> NodeConfig {
        NodeConfig {
            grpc: GrpcServerConfig {
                host: "127.0.0.1".into(),
                port,
                // Drain immediately; there is no load balancer to wait for.
                shutdown_grace: std::time::Duration::ZERO,
                ..Default::default()
            },
            database: DatabaseConfig::default(),
        }
    }

    /// The node must come back on its own once the shutdown signal fires. Before this, `serve`
    /// ran until the process was killed, so SIGTERM cut in-flight work instead of draining it.
    #[tokio::test]
    async fn entrypoint_returns_once_the_shutdown_signal_fires() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let node = Node::init(test_config(free_port())).await.unwrap();

        let running = tokio::spawn(node.entrypoint(async {
            let _ = rx.await;
        }));

        // Let the server reach its accept loop before asking it to stop.
        tokio::task::yield_now().await;
        tx.send(()).unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), running)
            .await
            .expect("entrypoint must return after the shutdown signal")
            .unwrap();

        assert!(result.is_ok(), "a signalled shutdown is not a failure: {result:?}");
    }

    /// A fatal server error must reach the caller rather than being logged and swallowed,
    /// otherwise the process exits 0 and `restart: on-failure` never fires.
    #[tokio::test]
    async fn entrypoint_returns_the_error_that_brought_the_server_down() {
        // Hold the port so that the node's own bind is guaranteed to fail.
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();

        let node = Node::init(test_config(port)).await.unwrap();

        let err = node.entrypoint(std::future::pending()).await.unwrap_err();

        assert!(err.to_string().contains("Server error"), "unexpected error: {err}");
    }
}

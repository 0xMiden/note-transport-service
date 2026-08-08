use std::time::Duration;

use clap::Parser;
use miden_note_transport_node::database::DatabaseConfig;
use miden_note_transport_node::logging::{TracingConfig, setup_tracing};
use miden_note_transport_node::node::grpc::GrpcServerConfig;
use miden_note_transport_node::{Node, NodeConfig, Result, shutdown};
use tracing::info;

#[derive(Parser)]
#[command(name = "miden-note-transport-node")]
#[command(about = "Miden Transport Node - Canonical transport layer for private notes")]
struct Args {
    /// Host to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind to
    #[arg(long, default_value = "57292")]
    port: u16,

    /// Database URL
    #[arg(long, default_value = ":memory:")]
    database_url: String,

    /// Retention period in days
    #[arg(long, default_value = "30")]
    retention_days: u32,

    /// Maximum note size in bytes
    #[arg(long, default_value = "512000")]
    max_note_size: usize,

    /// Maximum number of concurrent connections
    #[arg(long, default_value = "4096")]
    max_connections: usize,

    /// Connection timeout in seconds
    #[arg(long, default_value = "4")]
    request_timeout: usize,

    /// Seconds to keep serving after reporting `NOT_SERVING` on shutdown
    ///
    /// Should cover at least one health-check interval of whatever load balancer fronts the
    /// service, so it stops routing here before the node stops accepting. 0 drains immediately.
    #[arg(long, default_value = "12")]
    shutdown_grace_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Setup tracing. OpenTelemetry export turns on when a standard OTLP
    // endpoint env var is set (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT or
    // OTEL_EXPORTER_OTLP_ENDPOINT).
    let tracing_cfg = TracingConfig::from_otel_env();
    let telemetry = setup_tracing(tracing_cfg.clone())?;

    info!("Starting Miden Transport Node...");
    info!("Host: {}", args.host);
    info!("Port: {}", args.port);
    info!("Database: {}", args.database_url);
    info!("Max note size: {} bytes", args.max_note_size);
    info!("Retention days: {}", args.retention_days);
    info!(
        "Telemetry: OpenTelemetry={}, JSON={}",
        tracing_cfg.otel.is_enabled(),
        tracing_cfg.json_format
    );

    // Create Node config
    let config = NodeConfig {
        grpc: GrpcServerConfig {
            host: args.host,
            port: args.port,
            max_note_size: args.max_note_size,
            max_connections: args.max_connections,
            request_timeout: args.request_timeout,
            shutdown_grace: Duration::from_secs(args.shutdown_grace_secs),
        },
        database: DatabaseConfig {
            url: args.database_url,
            retention_days: args.retention_days,
        },
    };

    // Run Node until a shutdown signal arrives
    let node = Node::init(config).await?;
    let result = node.entrypoint(shutdown::signal()).await;

    // Flush buffered spans and metrics last, so everything the shutdown itself emitted is
    // exported rather than dying with the process.
    telemetry.shutdown();
    info!("Shutdown complete");

    result
}

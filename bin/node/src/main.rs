use clap::Parser;
use miden_note_transport_node::database::DatabaseConfig;
use miden_note_transport_node::logging::{TracingConfig, setup_tracing};
use miden_note_transport_node::node::grpc::{GrpcServerConfig, RateLimitConfig};
use miden_note_transport_node::{Node, NodeConfig, Result};
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

    // Rate limiting settings
    /// Rate limit: requests per second per IP
    #[arg(long, default_value = "50")]
    rate_limit_rps: u32,

    /// Rate limit: burst size (allows temporary spikes)
    #[arg(long, default_value = "100")]
    rate_limit_burst: u32,

    // TCP settings
    /// TCP keepalive interval in seconds (0 to disable)
    #[arg(long, default_value = "60")]
    tcp_keepalive: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Setup tracing
    let tracing_cfg = TracingConfig::from_env();
    setup_tracing(tracing_cfg.clone())?;

    info!("Starting Miden Transport Node...");
    info!("Host: {}", args.host);
    info!("Port: {}", args.port);
    info!("Database: {}", args.database_url);
    info!("Max note size: {} bytes", args.max_note_size);
    info!("Retention days: {}", args.retention_days);
    info!("Rate limit: {} req/s, burst: {}", args.rate_limit_rps, args.rate_limit_burst);
    info!("TCP keepalive: {}s", args.tcp_keepalive);
    info!(
        "Telemetry: OpenTelemetry={}, JSON={}",
        tracing_cfg.otel.is_enabled(),
        tracing_cfg.json_format
    );

    // Helper to convert 0 to None for optional duration settings
    let opt_nonzero = |v: u64| if v == 0 { None } else { Some(v) };

    // Create Node config
    let config = NodeConfig {
        grpc: GrpcServerConfig {
            host: args.host,
            port: args.port,
            max_note_size: args.max_note_size,
            max_connections: args.max_connections,
            request_timeout: args.request_timeout,
            rate_limit: RateLimitConfig {
                requests_per_second: args.rate_limit_rps,
                burst_size: args.rate_limit_burst,
            },
            tcp_keepalive_secs: opt_nonzero(args.tcp_keepalive),
        },
        database: DatabaseConfig {
            url: args.database_url,
            retention_days: args.retention_days,
        },
    };

    // Run Node
    let node = Node::init(config).await?;
    node.entrypoint().await;

    Ok(())
}

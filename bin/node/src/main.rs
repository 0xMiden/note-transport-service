use std::num::NonZeroU32;

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
    /// Rate limit: sustained requests per second per IP
    #[arg(long, default_value = "50")]
    rate_limit_rps: NonZeroU32,

    /// Rate limit: burst size (max tokens in the bucket)
    #[arg(long, default_value = "100")]
    rate_limit_burst: NonZeroU32,

    /// Trust `X-Forwarded-For` / `X-Real-IP` headers when identifying the client.
    /// Only enable when the server sits behind a trusted reverse proxy.
    #[arg(long, default_value_t = false)]
    rate_limit_trust_forwarded_headers: bool,

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
    info!(
        "Rate limit: {} req/s, burst: {}, trust forwarded headers: {}",
        args.rate_limit_rps, args.rate_limit_burst, args.rate_limit_trust_forwarded_headers,
    );
    if args.tcp_keepalive == 0 {
        info!("TCP keepalive: disabled");
    } else {
        info!("TCP keepalive: {}s", args.tcp_keepalive);
    }
    info!(
        "Telemetry: OpenTelemetry={}, JSON={}",
        tracing_cfg.otel.is_enabled(),
        tracing_cfg.json_format
    );

    let tcp_keepalive_secs = (args.tcp_keepalive != 0).then_some(args.tcp_keepalive);

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
                trust_forwarded_headers: args.rate_limit_trust_forwarded_headers,
            },
            tcp_keepalive_secs,
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

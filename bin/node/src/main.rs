use clap::{Args, Parser, Subcommand};
use miden_note_transport_node::database::{Database, DatabaseConfig};
use miden_note_transport_node::logging::{TracingConfig, setup_tracing};
use miden_note_transport_node::metrics::Metrics;
use miden_note_transport_node::node::grpc::GrpcServerConfig;
use miden_note_transport_node::{Node, NodeConfig, Result};
use tracing::info;

#[derive(Parser)]
#[command(name = "miden-note-transport-node")]
#[command(about = "Miden note transport service")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve gRPC and gRPC-Web traffic.
    Serve(ServeArgs),
    /// Apply database schema migrations and exit.
    Migrate(DatabaseArgs),
    /// Delete one bounded batch of expired notes and exit.
    Cleanup(CleanupArgs),
}

#[derive(Args)]
struct DatabaseArgs {
    /// SQLite database path.
    #[arg(long, env = "MNT_DATABASE_URL")]
    database_url: String,
}

#[derive(Args)]
struct CleanupArgs {
    #[command(flatten)]
    database: DatabaseArgs,

    #[arg(long, env = "MNT_RETENTION_DAYS", default_value = "30")]
    retention_days: u32,

    #[arg(long, env = "MNT_CLEANUP_MAX_ROWS", default_value = "1000")]
    max_rows: u32,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    database: DatabaseArgs,

    #[arg(long, env = "MNT_LISTEN", default_value = "127.0.0.1:57292")]
    listen: std::net::SocketAddr,

    #[arg(long, env = "MNT_MAX_NOTE_SIZE", default_value = "512000")]
    max_note_size: usize,

    #[arg(long, env = "MNT_MAX_REQUESTS", default_value = "4096")]
    max_requests: usize,

    #[arg(long, env = "MNT_REQUEST_TIMEOUT", default_value = "4")]
    request_timeout: usize,

    /// Maximum number of live `StreamNotes` requests.
    #[arg(long, default_value = "1024")]
    max_streams: usize,

    /// Maximum bytes retained for note headers and encrypted details.
    #[arg(long, env = "MNT_MAX_STORAGE_BYTES")]
    max_storage_bytes: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tracing_config = TracingConfig::from_otel_env();
    let _tracing_guard = setup_tracing(tracing_config)?;

    match cli.command {
        Command::Migrate(args) => {
            Database::migrate(&DatabaseConfig::new(args.database_url)).await?;
            info!("Database migrations completed");
        },
        Command::Cleanup(args) => {
            let database = Database::connect(
                DatabaseConfig::new(args.database.database_url),
                Metrics::default().db,
            )
            .await?;
            let deleted = database.cleanup_old_notes(args.retention_days, args.max_rows).await?;
            info!(deleted, "Database cleanup completed");
        },
        Command::Serve(args) => {
            let config = NodeConfig {
                grpc: GrpcServerConfig {
                    listen: args.listen,
                    max_note_size: args.max_note_size,
                    max_requests: args.max_requests,
                    request_timeout: args.request_timeout,
                    max_storage_bytes: args.max_storage_bytes,
                    max_streams: args.max_streams,
                },
                database: DatabaseConfig::new(args.database.database_url),
            };
            Node::init(config).await?.entrypoint().await?;
        },
    }

    Ok(())
}

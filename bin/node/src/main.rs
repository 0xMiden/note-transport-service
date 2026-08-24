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
    /// Copy an offline SQLite database into empty PostgreSQL storage.
    Copy(CopyArgs),
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

    #[arg(long, default_value = "30")]
    retention_days: u32,

    #[arg(long, default_value = "1000")]
    max_rows: u32,
}

#[derive(Args)]
struct CopyArgs {
    /// Path to the stopped SQLite database.
    #[arg(long)]
    sqlite: String,

    /// URL of an empty, migrated PostgreSQL database.
    #[arg(long, env = "MNT_DATABASE_URL")]
    postgres: String,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    database: DatabaseArgs,

    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value = "57292")]
    port: u16,

    #[arg(long, default_value = "512000")]
    max_note_size: usize,

    #[arg(long, default_value = "4096")]
    max_connections: usize,

    #[arg(long, default_value = "4")]
    request_timeout: usize,

    /// Maximum bytes retained for note headers and encrypted details.
    #[arg(long, env = "MNT_MAX_STORAGE_BYTES")]
    max_storage_bytes: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tracing_config = TracingConfig::from_otel_env();
    setup_tracing(tracing_config)?;

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
        Command::Copy(args) => {
            let copied = Database::copy_sqlite_to_postgres(
                &DatabaseConfig::new(args.sqlite),
                &DatabaseConfig::new(args.postgres),
                Metrics::default().db,
            )
            .await?;
            info!(copied, "SQLite notes copied and verified");
        },
        Command::Serve(args) => {
            if args.max_note_size > miden_note_transport_node::database::FETCH_NOTES_MAX_BYTES {
                return Err(miden_note_transport_node::Error::Internal(format!(
                    "max note size cannot exceed {} bytes",
                    miden_note_transport_node::database::FETCH_NOTES_MAX_BYTES
                )));
            }
            let config = NodeConfig {
                grpc: GrpcServerConfig {
                    host: args.host,
                    port: args.port,
                    max_note_size: args.max_note_size,
                    max_connections: args.max_connections,
                    request_timeout: args.request_timeout,
                    max_storage_bytes: args.max_storage_bytes,
                },
                database: DatabaseConfig::new(args.database.database_url),
            };
            Node::init(config).await?.entrypoint().await;
        },
    }

    Ok(())
}

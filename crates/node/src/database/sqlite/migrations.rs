use diesel::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use tracing::instrument;

use crate::database::DatabaseError;

// The rebuild is automatically triggered by `build.rs` as described in
// <https://docs.rs/diesel_migrations/latest/diesel_migrations/macro.embed_migrations.html#automatic-rebuilds>.
pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("src/database/sqlite/migrations");

#[instrument(level = "debug", skip_all, err)]
pub fn apply_migrations(conn: &mut SqliteConnection) -> std::result::Result<(), DatabaseError> {
    let migrations = conn.pending_migrations(MIGRATIONS).expect("embedded migrations are valid");
    tracing::info!("Applying {} migration(s)", migrations.len());

    if let Err(e) = conn.run_pending_migrations(MIGRATIONS) {
        tracing::warn!("Failed to apply migration: {e:?}");
        return Err(DatabaseError::Migration(format!("Migration failed: {e}")));
    }

    Ok(())
}

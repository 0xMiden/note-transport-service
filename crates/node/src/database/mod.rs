mod error;
mod postgres;
mod sqlite;

use std::sync::Arc;

pub use self::error::DatabaseError;
use self::postgres::PostgresDatabase;
use self::sqlite::SqliteDatabase;
use crate::metrics::MetricsDatabase;
use crate::types::{NoteTag, StoredNote};

pub(crate) const FETCH_NOTES_MAX_ROWS: u32 = 500;
/// Hard upper bound for one stored envelope and one fetched page.
pub const FETCH_NOTES_MAX_BYTES: usize = 3 * 1024 * 1024;
pub(crate) const LEGACY_CURSOR_THRESHOLD: u64 = 1_000_000_000_000;

pub(crate) fn normalize_fetch_cursor(cursor: u64) -> u64 {
    if cursor > LEGACY_CURSOR_THRESHOLD { 0 } else { cursor }
}

/// Result of storing an opaque note envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreResult {
    /// The envelope was committed with a new cursor.
    Inserted,
    /// An identical envelope was already present.
    AlreadyPresent,
}

struct StorageSnapshot {
    notes: Vec<StoredNote>,
    next_cursor: i64,
    retained_bytes: i64,
}

#[async_trait::async_trait]
trait DatabaseBackend: Send + Sync {
    async fn store_note(
        &self,
        note: &StoredNote,
        max_retained_bytes: u64,
    ) -> Result<StoreResult, DatabaseError>;

    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
        max_rows: u32,
        max_bytes: usize,
    ) -> Result<Vec<StoredNote>, DatabaseError>;

    async fn cleanup_old_notes(
        &self,
        retention_days: u32,
        max_rows: u32,
    ) -> Result<u64, DatabaseError>;
}

/// Database connection configuration.
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// SQLite file path or PostgreSQL connection URL.
    pub url: String,
    allow_in_memory: bool,
}

impl DatabaseConfig {
    /// Create configuration for a persistent database.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), allow_in_memory: false }
    }

    #[cfg(any(test, feature = "testing"))]
    /// Create the explicit in-memory configuration used by tests.
    pub fn in_memory_for_tests() -> Self {
        Self {
            url: "sqlite::memory:".to_string(),
            allow_in_memory: true,
        }
    }

    fn is_postgres(&self) -> bool {
        self.url.starts_with("postgres://") || self.url.starts_with("postgresql://")
    }
}

#[derive(Clone)]
/// Backend-independent note storage handle.
pub struct Database {
    backend: Arc<dyn DatabaseBackend>,
}

impl Database {
    /// Connect to an already-migrated database.
    pub async fn connect(
        config: DatabaseConfig,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError> {
        let backend: Arc<dyn DatabaseBackend> = if config.is_postgres() {
            Arc::new(PostgresDatabase::connect(&config.url, metrics).await?)
        } else {
            Arc::new(SqliteDatabase::connect(&config.url, config.allow_in_memory, metrics).await?)
        };
        Ok(Self { backend })
    }

    /// Apply all schema migrations. Serving never calls this method.
    pub async fn migrate(config: &DatabaseConfig) -> Result<(), DatabaseError> {
        if config.is_postgres() {
            PostgresDatabase::migrate(&config.url).await
        } else {
            SqliteDatabase::migrate(&config.url, config.allow_in_memory).await
        }
    }

    /// Copy a stopped SQLite database into an empty PostgreSQL database.
    pub async fn copy_sqlite_to_postgres(
        sqlite: &DatabaseConfig,
        postgres: &DatabaseConfig,
        metrics: MetricsDatabase,
    ) -> Result<u64, DatabaseError> {
        if sqlite.is_postgres() || !postgres.is_postgres() {
            return Err(DatabaseError::Configuration(
                "copy requires a SQLite source and PostgreSQL destination".to_string(),
            ));
        }
        let source = SqliteDatabase::connect(&sqlite.url, false, metrics.clone()).await?;
        let snapshot = source.export_all().await?;
        let count = snapshot.notes.len() as u64;
        let destination = PostgresDatabase::connect(&postgres.url, metrics).await?;
        destination.import_all(&snapshot).await?;
        destination.verify_import(&snapshot).await?;
        Ok(count)
    }

    #[cfg(any(test, feature = "testing"))]
    /// Create and migrate an isolated in-memory SQLite backend for tests.
    pub async fn connect_for_test(metrics: MetricsDatabase) -> Result<Self, DatabaseError> {
        let config = DatabaseConfig::in_memory_for_tests();
        let backend = SqliteDatabase::connect_and_migrate_for_test(&config.url, metrics).await?;
        Ok(Self { backend: Arc::new(backend) })
    }

    /// Store an envelope or recognize an identical retry.
    pub async fn store_note(
        &self,
        note: &StoredNote,
        max_retained_bytes: u64,
    ) -> Result<StoreResult, DatabaseError> {
        self.backend.store_note(note, max_retained_bytes).await
    }

    /// Fetch a bounded page for one tag.
    pub async fn fetch_notes(
        &self,
        tag: NoteTag,
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.fetch_notes_by_tags(&[tag], cursor).await
    }

    /// Fetch a bounded page matching any supplied tag.
    pub async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.backend
            .fetch_notes_by_tags(
                tags,
                normalize_fetch_cursor(cursor),
                FETCH_NOTES_MAX_ROWS,
                FETCH_NOTES_MAX_BYTES,
            )
            .await
    }

    /// Delete at most `max_rows` expired notes.
    pub async fn cleanup_old_notes(
        &self,
        retention_days: u32,
        max_rows: u32,
    ) -> Result<u64, DatabaseError> {
        self.backend.cleanup_old_notes(retention_days, max_rows).await
    }
}

pub(crate) fn envelope_digest(note: &StoredNote) -> [u8; 32] {
    use miden_protocol::utils::serde::Serializable;

    let header = note.header.to_bytes();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"miden-note-transport-envelope-v1");
    hasher.update(&(header.len() as u64).to_le_bytes());
    hasher.update(&header);
    hasher.update(&(note.details.len() as u64).to_le_bytes());
    hasher.update(&note.details);
    match note.after_block_num {
        Some(block_num) => {
            hasher.update(&[1]);
            hasher.update(&block_num.to_le_bytes());
        },
        None => {
            hasher.update(&[0]);
        },
    }
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use miden_protocol::crypto::dsa::eddsa_25519_sha512::KeyExchangeKey;
    use miden_protocol::crypto::ies::SealingKey;
    use miden_protocol::utils::serde::{Deserializable, Serializable};

    use super::*;
    use crate::metrics::Metrics;
    use crate::test_utils::{TAG_LOCAL_ANY, test_note_header};

    fn note(details: &[u8]) -> StoredNote {
        let header = test_note_header();
        let secret = KeyExchangeKey::read_from_bytes(&[7_u8; 32]).unwrap();
        let sealing_key = SealingKey::X25519XChaCha20Poly1305(secret.public_key());
        let details = sealing_key
            .seal_bytes_with_associated_data(&mut rand::rng(), details, &header.to_bytes())
            .unwrap()
            .to_bytes();
        StoredNote {
            header,
            details,
            created_at: Utc::now(),
            seq: 0,
            after_block_num: None,
        }
    }

    async fn sqlite() -> Database {
        Database::connect_for_test(Metrics::default().db).await.unwrap()
    }

    async fn backend_contract(db: Database) {
        let first = note(&[1]);
        assert_eq!(db.store_note(&first, u64::MAX).await.unwrap(), StoreResult::Inserted);
        assert_eq!(db.store_note(&first, 0).await.unwrap(), StoreResult::AlreadyPresent);

        let mut variant = first.clone();
        variant.details = vec![2];
        assert_eq!(first.header.id(), variant.header.id());
        let first_size = (first.header.to_bytes().len() + first.details.len()) as u64;
        assert!(matches!(
            db.store_note(&variant, first_size).await,
            Err(DatabaseError::Capacity(_))
        ));
        assert_eq!(db.store_note(&variant, u64::MAX).await.unwrap(), StoreResult::Inserted);

        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.len(), 2);
        assert!(fetched[0].seq < fetched[1].seq);

        let legacy_cursor = LEGACY_CURSOR_THRESHOLD + 1;
        assert_eq!(db.fetch_notes(TAG_LOCAL_ANY.into(), legacy_cursor).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sqlite_backend_contract() {
        backend_contract(sqlite().await).await;
    }

    #[tokio::test]
    async fn postgres_backend_and_sqlite_copy_contract() {
        let Ok(url) = std::env::var("MNT_TEST_POSTGRES_URL") else {
            return;
        };
        let config = DatabaseConfig::new(&url);
        Database::migrate(&config).await.unwrap();
        reset_postgres(&url).await;

        let sqlite_file = tempfile::NamedTempFile::new().unwrap();
        let sqlite_url = sqlite_file.path().to_string_lossy().into_owned();
        let sqlite_config = DatabaseConfig::new(&sqlite_url);
        Database::migrate(&sqlite_config).await.unwrap();
        let sqlite = Database::connect(sqlite_config.clone(), Metrics::default().db).await.unwrap();
        sqlite.store_note(&note(&[1]), u64::MAX).await.unwrap();
        sqlite.store_note(&note(&[2]), u64::MAX).await.unwrap();

        let copied =
            Database::copy_sqlite_to_postgres(&sqlite_config, &config, Metrics::default().db)
                .await
                .unwrap();
        assert_eq!(copied, 2);
        let postgres = Database::connect(config.clone(), Metrics::default().db).await.unwrap();
        assert_eq!(postgres.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap().len(), 2);

        drop(postgres);
        reset_postgres(&url).await;
        sqlite.cleanup_old_notes(0, 2).await.unwrap();
        assert_eq!(
            Database::copy_sqlite_to_postgres(&sqlite_config, &config, Metrics::default().db)
                .await
                .unwrap(),
            0
        );
        let postgres = Database::connect(config.clone(), Metrics::default().db).await.unwrap();
        postgres.store_note(&note(&[3]), u64::MAX).await.unwrap();
        let fetched = postgres.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched[0].seq, 3);

        drop(postgres);
        reset_postgres(&url).await;
        backend_contract(Database::connect(config, Metrics::default().db).await.unwrap()).await;
    }

    async fn reset_postgres(url: &str) {
        let pool = sqlx::PgPool::connect(url).await.unwrap();
        sqlx::query("TRUNCATE TABLE notes").execute(&pool).await.unwrap();
        sqlx::query(
            "UPDATE storage_metadata SET next_cursor = 1, retained_bytes = 0 \
             WHERE singleton = TRUE",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sqlite_reads_and_cleanup_are_bounded() {
        let db = sqlite().await;
        let mut first = note(&[1; 64]);
        first.created_at = Utc::now() - chrono::Duration::days(2);
        let mut second = first.clone();
        second.details = vec![2; 64];
        db.store_note(&first, u64::MAX).await.unwrap();
        db.store_note(&second, u64::MAX).await.unwrap();

        let fetched = db
            .backend
            .fetch_notes_by_tags(&[TAG_LOCAL_ANY.into()], 0, 500, 1)
            .await
            .unwrap();
        assert!(fetched.is_empty());
        assert_eq!(db.cleanup_old_notes(1, 1).await.unwrap(), 1);
        assert_eq!(db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn envelopes_larger_than_a_fetch_page_are_rejected() {
        let db = sqlite().await;
        let mut oversized = note(&[1]);
        oversized.details = vec![0; FETCH_NOTES_MAX_BYTES + 1];

        assert!(matches!(
            db.store_note(&oversized, u64::MAX).await,
            Err(DatabaseError::Capacity(_))
        ));
    }

    #[test]
    fn digest_includes_the_complete_envelope() {
        let plain = note(&[1]);
        let mut different_context = plain.clone();
        different_context.after_block_num = Some(1);
        assert_ne!(envelope_digest(&plain), envelope_digest(&different_context));
    }
}

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

/// Result of storing a note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreResult {
    /// The note was committed with a new cursor.
    Inserted,
    /// The note ID was already present.
    AlreadyPresent,
}

/// A bounded page of stored notes.
#[derive(Debug)]
pub struct FetchPage {
    /// Notes that fit within the page limits.
    pub notes: Vec<StoredNote>,
    /// Whether another request may return more notes.
    pub has_more: bool,
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
    ) -> Result<FetchPage, DatabaseError>;

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

    #[cfg(any(test, feature = "testing"))]
    /// Create and migrate an isolated in-memory SQLite backend for tests.
    pub async fn connect_for_test(metrics: MetricsDatabase) -> Result<Self, DatabaseError> {
        let config = DatabaseConfig::in_memory_for_tests();
        let backend = SqliteDatabase::connect_and_migrate_for_test(&config.url, metrics).await?;
        Ok(Self { backend: Arc::new(backend) })
    }

    /// Store a note or recognize a retry by note ID.
    pub async fn store_note(
        &self,
        note: &StoredNote,
        max_retained_bytes: u64,
    ) -> Result<StoreResult, DatabaseError> {
        self.backend.store_note(note, max_retained_bytes).await
    }

    /// Fetch a bounded page for one tag.
    pub async fn fetch_notes(&self, tag: NoteTag, cursor: u64) -> Result<FetchPage, DatabaseError> {
        self.fetch_notes_by_tags(&[tag], cursor).await
    }

    /// Fetch a bounded page matching any supplied tag.
    pub async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<FetchPage, DatabaseError> {
        self.backend
            .fetch_notes_by_tags(tags, cursor, FETCH_NOTES_MAX_ROWS, FETCH_NOTES_MAX_BYTES)
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

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use miden_protocol::note::{NoteDetails, NoteHeader};
    use miden_protocol::utils::serde::Serializable;

    use super::*;
    use crate::metrics::Metrics;
    use crate::test_utils::{TAG_LOCAL_ANY, test_note};

    fn note() -> StoredNote {
        let note = test_note();
        StoredNote {
            header: NoteHeader::from(&note),
            details: NoteDetails::from(note).to_bytes(),
            created_at: Utc::now(),
            seq: 0,
            after_block_num: None,
        }
    }

    async fn sqlite() -> Database {
        Database::connect_for_test(Metrics::default().db).await.unwrap()
    }

    async fn backend_contract(db: &Database) {
        let first = note();
        assert_eq!(db.store_note(&first, u64::MAX).await.unwrap(), StoreResult::Inserted);
        assert_eq!(db.store_note(&first, 0).await.unwrap(), StoreResult::AlreadyPresent);

        let mut retry = first.clone();
        retry.after_block_num = Some(1);
        assert_eq!(db.store_note(&retry, 0).await.unwrap(), StoreResult::AlreadyPresent);

        let second = note();
        let first_size = (first.header.to_bytes().len() + first.details.len()) as u64;
        assert!(matches!(
            db.store_note(&second, first_size).await,
            Err(DatabaseError::Capacity(_))
        ));
        assert_eq!(db.store_note(&second, u64::MAX).await.unwrap(), StoreResult::Inserted);

        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.notes.len(), 2);
        assert!(!fetched.has_more);
        assert!(fetched.notes[0].seq < fetched.notes[1].seq);
    }

    #[tokio::test]
    async fn sqlite_backend_contract() {
        backend_contract(&sqlite().await).await;
    }

    #[tokio::test]
    async fn postgres_backend_contract() {
        let Ok(url) = std::env::var("MNT_TEST_POSTGRES_URL") else {
            return;
        };
        let config = DatabaseConfig::new(&url);
        Database::migrate(&config).await.unwrap();
        reset_postgres(&url).await;

        let postgres = Database::connect(config, Metrics::default().db).await.unwrap();
        backend_contract(&postgres).await;
        let mut expired = note();
        expired.created_at = Utc::now() - chrono::Duration::days(2);
        postgres.store_note(&expired, u64::MAX).await.unwrap();
        assert_eq!(postgres.cleanup_old_notes(1, 1).await.unwrap(), 1);
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
        let mut first = note();
        first.details = vec![1; 64];
        first.created_at = Utc::now() - chrono::Duration::days(2);
        let mut second = note();
        second.details = vec![2; 64];
        db.store_note(&first, u64::MAX).await.unwrap();
        db.store_note(&second, u64::MAX).await.unwrap();

        let one_envelope = first.header.to_bytes().len() + first.details.len();

        let fetched = db
            .backend
            .fetch_notes_by_tags(&[TAG_LOCAL_ANY.into()], 0, 500, one_envelope)
            .await
            .unwrap();
        assert_eq!(fetched.notes.len(), 1);
        assert!(fetched.has_more);
        assert_eq!(db.cleanup_old_notes(1, 1).await.unwrap(), 1);
        assert_eq!(db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap().notes.len(), 1);
    }

    #[tokio::test]
    async fn sqlite_fetch_reports_more_rows() {
        let db = sqlite().await;
        db.store_note(&note(), u64::MAX).await.unwrap();
        db.store_note(&note(), u64::MAX).await.unwrap();

        let first = db
            .backend
            .fetch_notes_by_tags(&[TAG_LOCAL_ANY.into()], 0, 1, FETCH_NOTES_MAX_BYTES)
            .await
            .unwrap();
        assert_eq!(first.notes.len(), 1);
        assert!(first.has_more);

        let second = db
            .backend
            .fetch_notes_by_tags(
                &[TAG_LOCAL_ANY.into()],
                first.notes[0].seq.try_into().unwrap(),
                1,
                FETCH_NOTES_MAX_BYTES,
            )
            .await
            .unwrap();
        assert_eq!(second.notes.len(), 1);
        assert!(!second.has_more);
    }

    #[tokio::test]
    async fn envelopes_larger_than_a_fetch_page_are_rejected() {
        let db = sqlite().await;
        let mut oversized = note();
        oversized.details = vec![0; FETCH_NOTES_MAX_BYTES + 1];

        assert!(matches!(
            db.store_note(&oversized, u64::MAX).await,
            Err(DatabaseError::Capacity(_))
        ));
    }
}

mod error;
mod postgres;
mod sqlite;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use self::error::DatabaseError;
use self::postgres::PostgresDatabase;
use self::sqlite::SqliteDatabase;
use crate::metrics::MetricsDatabase;
use crate::types::{NoteTag, StoredNote};

pub(crate) const FETCH_NOTES_MAX_ROWS: u32 = 500;
/// Hard upper bound for one stored envelope and one fetched page.
pub const FETCH_NOTES_MAX_BYTES: usize = 3 * 1024 * 1024;

#[cfg(test)]
pub(crate) static POSTGRES_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Result of storing an opaque note envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreResult {
    /// The envelope was committed with a new cursor.
    Inserted,
    /// An identical envelope was already present.
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

/// Storage change state observed by streaming readers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DatabaseWatch {
    generation: u64,
    ready: bool,
}

impl DatabaseWatch {
    const fn new(ready: bool) -> Self {
        Self { generation: 0, ready }
    }

    fn advance(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(crate) const fn is_ready(self) -> bool {
        self.ready
    }
}

#[derive(Clone)]
struct DatabaseNotifications {
    inner: Arc<DatabaseNotificationsInner>,
}

struct DatabaseNotificationsInner {
    ready: AtomicBool,
    tags: Mutex<HashMap<NoteTag, tokio::sync::watch::Sender<DatabaseWatch>>>,
}

impl DatabaseNotifications {
    fn new() -> Self {
        Self {
            inner: Arc::new(DatabaseNotificationsInner {
                ready: AtomicBool::new(true),
                tags: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn subscribe(&self, tag: NoteTag) -> tokio::sync::watch::Receiver<DatabaseWatch> {
        let mut tags = self.inner.tags.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        tags.retain(|_, sender| sender.receiver_count() > 0);
        tags.entry(tag)
            .or_insert_with(|| {
                tokio::sync::watch::channel(DatabaseWatch::new(
                    self.inner.ready.load(Ordering::Acquire),
                ))
                .0
            })
            .subscribe()
    }

    fn notify(&self, tag: NoteTag) {
        let mut tags = self.inner.tags.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tags.get(&tag).is_some_and(|sender| sender.receiver_count() == 0) {
            tags.remove(&tag);
        } else if let Some(sender) = tags.get(&tag) {
            sender.send_modify(DatabaseWatch::advance);
        }
    }

    fn set_ready(&self, ready: bool) {
        self.inner.ready.store(ready, Ordering::Release);
        let mut tags = self.inner.tags.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        tags.retain(|_, sender| {
            if sender.receiver_count() == 0 {
                false
            } else {
                sender.send_modify(|state| {
                    state.ready = ready;
                    state.advance();
                });
                true
            }
        });
    }

    fn is_ready(&self) -> bool {
        self.inner.ready.load(Ordering::Acquire)
    }
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

    fn subscribe(&self, tag: NoteTag) -> tokio::sync::watch::Receiver<DatabaseWatch>;

    async fn is_ready(&self) -> bool;
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

    /// Store an envelope or recognize an identical retry.
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

    /// Subscribe to committed storage changes.
    pub(crate) fn subscribe(&self, tag: NoteTag) -> tokio::sync::watch::Receiver<DatabaseWatch> {
        self.backend.subscribe(tag)
    }

    /// Check whether storage can serve requests and deliver commit signals.
    pub async fn is_ready(&self) -> bool {
        self.backend.is_ready().await
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

pub(crate) fn advance_cursor(notes: &[StoredNote], cursor: u64) -> Result<u64, DatabaseError> {
    notes.iter().try_fold(cursor, |cursor, note| {
        let seq = u64::try_from(note.seq)
            .map_err(|_| DatabaseError::Deserialization("negative note cursor".to_string()))?;
        Ok(cursor.max(seq))
    })
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

    async fn backend_contract(db: &Database) {
        let mut changes = db.subscribe(TAG_LOCAL_ANY.into());
        let mut unrelated = db.subscribe((TAG_LOCAL_ANY + 1).into());
        let first = note(&[1]);
        assert_eq!(db.store_note(&first, u64::MAX).await.unwrap(), StoreResult::Inserted);
        tokio::time::timeout(std::time::Duration::from_secs(1), changes.changed())
            .await
            .expect("the committed note did not notify subscribers")
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), unrelated.changed())
                .await
                .is_err(),
            "a write woke a subscriber for another tag"
        );
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
        let _guard = POSTGRES_TEST_LOCK.lock().await;
        let config = DatabaseConfig::new(&url);
        Database::migrate(&config).await.unwrap();
        reset_postgres(&url).await;

        let postgres = Database::connect(config, Metrics::default().db).await.unwrap();
        backend_contract(&postgres).await;
        let mut expired = note(&[4]);
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
        let mut first = note(&[1; 64]);
        first.created_at = Utc::now() - chrono::Duration::days(2);
        let mut second = first.clone();
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
        db.store_note(&note(&[1]), u64::MAX).await.unwrap();
        db.store_note(&note(&[2]), u64::MAX).await.unwrap();

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

mod error;
mod maintenance;
mod sqlite;

pub use self::error::DatabaseError;
pub use self::maintenance::DatabaseMaintenance;
use self::sqlite::SqliteDatabase;
use crate::metrics::MetricsDatabase;
use crate::types::{NoteId, NoteTag, StoredNote};

/// Database operations
#[async_trait::async_trait]
pub trait DatabaseBackend: Send + Sync {
    /// Connect to the database
    async fn connect(
        config: DatabaseConfig,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError>
    where
        Self: Sized;

    /// Store a new note
    async fn store_note(&self, note: &StoredNote) -> Result<(), DatabaseError>;

    /// Fetch notes by tag
    async fn fetch_notes(
        &self,
        tag: NoteTag,
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError>;

    /// Fetch notes matching ANY of a set of tags, in a single DB snapshot.
    ///
    /// This is the preferred multi-tag query — running per-tag queries back
    /// to back reopens a race where a concurrent INSERT can land between two
    /// per-tag queries and get leapfrogged by the cursor advance.
    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError>;

    /// Get statistics about the database
    async fn get_stats(&self) -> Result<(u64, u64), DatabaseError>;

    /// Clean up old notes based on retention policy
    async fn cleanup_old_notes(&self, retention_days: u32) -> Result<u64, DatabaseError>;

    /// Check if a note exists
    async fn note_exists(&self, note_id: NoteId) -> Result<bool, DatabaseError>;
}

/// Database manager for the transport layer
pub struct Database {
    backend: Box<dyn DatabaseBackend>,
}

/// [`Database`] configuration
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Database URL
    pub url: String,
    /// Retention period in days
    pub retention_days: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: ":memory:".to_string(),
            retention_days: 30,
        }
    }
}

impl Database {
    /// Connect to a database (with `SQLite` backend)
    pub async fn connect(
        config: DatabaseConfig,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError> {
        let backend = SqliteDatabase::connect(config, metrics).await?;
        Ok(Self { backend: Box::new(backend) })
    }

    /// Store a new note
    pub async fn store_note(&self, note: &StoredNote) -> Result<(), DatabaseError> {
        self.backend.store_note(note).await?;
        Ok(())
    }

    /// Fetch notes by tag with cursor-based pagination
    pub async fn fetch_notes(
        &self,
        tag: NoteTag,
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.backend.fetch_notes(tag, cursor).await
    }

    /// Fetch notes matching ANY of a set of tags, in a single DB snapshot.
    pub async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.backend.fetch_notes_by_tags(tags, cursor).await
    }

    /// Get statistics about the database
    pub async fn get_stats(&self) -> Result<(u64, u64), DatabaseError> {
        self.backend.get_stats().await
    }

    /// Clean up old notes based on retention policy
    pub async fn cleanup_old_notes(&self, retention_days: u32) -> Result<u64, DatabaseError> {
        self.backend.cleanup_old_notes(retention_days).await
    }

    /// Check if a note exists
    pub async fn note_exists(&self, note_id: NoteId) -> Result<bool, DatabaseError> {
        self.backend.note_exists(note_id).await
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::metrics::Metrics;
    use crate::test_utils::{TAG_LOCAL_ANY, test_note_header, test_note_header_with_tag};

    #[tokio::test]
    async fn test_sqlite_database() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        let note = StoredNote {
            header: test_note_header(),
            details: vec![1, 2, 3, 4],
            created_at: Utc::now(),
            seq: 0, // ignored on INSERT
        };

        db.store_note(&note).await.unwrap();

        // Cursor is now seq-based; 0 fetches everything.
        let fetched_notes = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched_notes.len(), 1);
        assert_eq!(fetched_notes[0].header.id(), note.header.id());
        assert!(fetched_notes[0].seq > 0);

        // Test note exists
        assert!(db.note_exists(note.header.id()).await.unwrap());

        // Test stats
        let (total_notes, total_tags) = db.get_stats().await.unwrap();
        assert_eq!(total_notes, 1);
        assert_eq!(total_tags, 1);
    }

    #[tokio::test]
    async fn test_seq_assigned_monotonically_in_insert_order() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        let first = StoredNote {
            header: test_note_header(),
            details: vec![1],
            created_at: Utc::now(),
            seq: 0,
        };
        db.store_note(&first).await.unwrap();

        let second = StoredNote {
            header: test_note_header(),
            details: vec![2],
            created_at: Utc::now(),
            seq: 0,
        };
        db.store_note(&second).await.unwrap();

        // Fetch everything and assert INSERT order = read order = seq ascending.
        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.len(), 2);
        assert!(fetched[0].seq < fetched[1].seq, "expected monotonic seq; got {} then {}", fetched[0].seq, fetched[1].seq);
        assert_eq!(fetched[0].details, vec![1]);
        assert_eq!(fetched[1].details, vec![2]);

        // Cursor between the two seqs returns only the second.
        let mid_cursor = fetched[0].seq as u64;
        let after_first = db.fetch_notes(TAG_LOCAL_ANY.into(), mid_cursor).await.unwrap();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].details, vec![2]);
    }

    #[tokio::test]
    async fn test_concurrent_store_fetch_sees_all_rows() {
        // Regression test for the `:memory:` pool-isolation bug: when the pool
        // had max_size>1 and the URL was `:memory:`, writes and reads could
        // land on different connections and each connection had its own
        // isolated in-memory DB. Result: writes silently split across pool
        // connections, fetches only saw a fraction of the actual data.
        //
        // With the pool clamped to size=1 for `:memory:`, all ops go to the
        // same connection and see the same DB.
        use std::sync::Arc;
        use tokio::task::JoinSet;

        const TAG_A: u32 = 0x3d9c_0000;
        const TAG_B: u32 = 0x47ac_0000;

        let db = Arc::new(
            Database::connect(DatabaseConfig::default(), Metrics::default().db).await.unwrap(),
        );

        // Spawn many concurrent writers — more than the old max_size=16 — so
        // that the bug would have fragmented writes across connections.
        let mut writers = JoinSet::new();
        for i in 0..40u32 {
            let db = db.clone();
            writers.spawn(async move {
                let tag = if i % 2 == 0 { TAG_A } else { TAG_B };
                db.store_note(&StoredNote {
                    header: test_note_header_with_tag(tag),
                    details: vec![i as u8],
                    created_at: Utc::now(),
                    seq: 0,
                })
                .await
                .unwrap();
            });
        }
        while writers.join_next().await.is_some() {}

        let fetched_a = db.fetch_notes(TAG_A.into(), 0).await.unwrap();
        let fetched_b = db.fetch_notes(TAG_B.into(), 0).await.unwrap();
        assert_eq!(fetched_a.len() + fetched_b.len(), 40, "all 40 concurrent writes should be visible");

        let (total, _) = db.get_stats().await.unwrap();
        assert_eq!(total, 40, "stats should reflect all 40 rows");
    }

    #[tokio::test]
    async fn test_fetch_notes_seq_cursor_filtering() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        let note = StoredNote {
            header: test_note_header(),
            details: vec![1, 2, 3, 4],
            created_at: Utc::now(),
            seq: 0, // ignored on INSERT
        };

        db.store_note(&note).await.unwrap();

        // cursor=0 is strictly before any assigned seq → should return the note
        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.len(), 1);
        let stored_seq = fetched[0].seq;
        assert!(stored_seq > 0, "expected seq > 0, got {stored_seq}");

        // cursor = the note's own seq → strictly-greater filter excludes it
        let after = db.fetch_notes(TAG_LOCAL_ANY.into(), stored_seq as u64).await.unwrap();
        assert_eq!(after.len(), 0);
    }
}

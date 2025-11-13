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

    /// Fetch notes by tags
    ///
    /// Fetched notes must be after the provided cursor, up to some limit of notes.
    /// If limit is None, no limit is applied.
    /// Notes from all tags are combined, ordered by timestamp globally, and the limit
    /// is applied to the combined set.
    async fn fetch_notes(
        &self,
        tags: &[NoteTag],
        cursor: u64,
        limit: Option<u32>,
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

    /// Fetch notes by tags with cursor-based pagination
    ///
    /// Notes from all tags are combined, ordered by timestamp globally, and the limit
    /// is applied to the combined set.
    pub async fn fetch_notes(
        &self,
        tags: &[NoteTag],
        cursor: u64,
        limit: Option<u32>,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.backend.fetch_notes(tags, cursor, limit).await
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
    use miden_objects::account::AccountId;
    use miden_objects::testing::account_id::ACCOUNT_ID_MAX_ZEROES;

    use super::*;
    use crate::metrics::Metrics;
    use crate::test_utils::{random_account_id, test_note_header};

    const TAG_LOCAL_ANY: u32 = 0xc000_0000;

    fn default_test_account_id() -> AccountId {
        AccountId::try_from(ACCOUNT_ID_MAX_ZEROES).unwrap()
    }

    #[tokio::test]
    async fn test_sqlite_database() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();
        let start = Utc::now();

        let note = StoredNote {
            header: test_note_header(default_test_account_id()),
            details: vec![1, 2, 3, 4],
            created_at: Utc::now(),
        };

        db.store_note(&note).await.unwrap();

        let cursor = start.timestamp_micros().try_into().unwrap();
        let fetched_notes = db.fetch_notes(&[TAG_LOCAL_ANY.into()], cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 1);
        assert_eq!(fetched_notes[0].header.id(), note.header.id());

        // Test note exists
        assert!(db.note_exists(note.header.id()).await.unwrap());

        // Test stats
        let (total_notes, total_tags) = db.get_stats().await.unwrap();
        assert_eq!(total_notes, 1);
        assert_eq!(total_tags, 1);
    }

    #[tokio::test]
    async fn test_fetch_notes_pagination() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        // Create a note with a specific received_at time
        let received_time = Utc::now();
        let note = StoredNote {
            header: test_note_header(default_test_account_id()),
            details: vec![1, 2, 3, 4],
            created_at: received_time,
        };

        db.store_note(&note).await.unwrap();

        // Fetch notes with cursor before the note was received - should return the note
        let before_cursor = (received_time - chrono::Duration::seconds(1))
            .timestamp_micros()
            .try_into()
            .unwrap();
        let fetched_notes =
            db.fetch_notes(&[TAG_LOCAL_ANY.into()], before_cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 1);
        assert_eq!(fetched_notes[0].header.id(), note.header.id());

        // Fetch notes with cursor after the note was received - should return empty
        let after_cursor = (received_time + chrono::Duration::seconds(1))
            .timestamp_micros()
            .try_into()
            .unwrap();
        let fetched_notes =
            db.fetch_notes(&[TAG_LOCAL_ANY.into()], after_cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 0);
    }

    #[tokio::test]
    async fn test_fetch_notes_limit() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();
        let start = Utc::now();

        // Create 5 notes with slightly different timestamps
        let mut note_ids = Vec::new();
        for i in 0..5u8 {
            let note = StoredNote {
                header: test_note_header(default_test_account_id()),
                details: vec![i],
                created_at: start + chrono::Duration::milliseconds(i64::from(i) * 10),
            };
            note_ids.push(note.header.id());
            db.store_note(&note).await.unwrap();
        }

        let cursor = 0;

        // Limit = 2
        let fetched_notes = db.fetch_notes(&[TAG_LOCAL_ANY.into()], cursor, Some(2)).await.unwrap();
        assert_eq!(fetched_notes.len(), 2);
        // Verify they are the first two notes in order
        assert_eq!(fetched_notes[0].header.id(), note_ids[0]);
        assert_eq!(fetched_notes[1].header.id(), note_ids[1]);

        // Limit larger than available notes
        let fetched_notes =
            db.fetch_notes(&[TAG_LOCAL_ANY.into()], cursor, Some(10)).await.unwrap();
        assert_eq!(fetched_notes.len(), 5);
        // Verify all notes are returned in order
        for (i, note) in fetched_notes.iter().enumerate() {
            assert_eq!(note.header.id(), note_ids[i]);
        }

        // No limit
        let fetched_notes = db.fetch_notes(&[TAG_LOCAL_ANY.into()], cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 5);
        // Verify all notes are returned in order
        for (i, note) in fetched_notes.iter().enumerate() {
            assert_eq!(note.header.id(), note_ids[i]);
        }

        // Limit = 0
        let fetched_notes = db.fetch_notes(&[TAG_LOCAL_ANY.into()], cursor, Some(0)).await.unwrap();
        assert_eq!(fetched_notes.len(), 0);
    }

    #[tokio::test]
    async fn test_fetch_notes_multiple_tags() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();
        let start = Utc::now();

        let account_id1 = random_account_id();
        let tag1 = NoteTag::from_account_id(account_id1);

        let account_id2 = random_account_id();
        let tag2 = NoteTag::from_account_id(account_id2);

        let account_id3 = random_account_id();
        let tag3 = NoteTag::from_account_id(account_id3);

        // Create notes with interleaved timestamps across 3 tags
        // Tag1: 0, 30, 60
        // Tag2: 10, 40, 70
        // Tag3: 20, 50, 80

        let mut tag1_note_ids = Vec::new();
        let mut tag2_note_ids = Vec::new();
        let mut tag3_note_ids = Vec::new();

        // Create notes for tag1 at 0, 30, 60
        for i in 0..3u8 {
            let note = StoredNote {
                header: test_note_header(account_id1),
                details: vec![i],
                created_at: start + chrono::Duration::milliseconds(i64::from(i) * 30),
            };
            tag1_note_ids.push(note.header.id());
            db.store_note(&note).await.unwrap();

            let note = StoredNote {
                header: test_note_header(account_id2),
                details: vec![i + 10],
                created_at: start + chrono::Duration::milliseconds(i64::from(i) * 30 + 10),
            };
            tag2_note_ids.push(note.header.id());
            db.store_note(&note).await.unwrap();

            let note = StoredNote {
                header: test_note_header(account_id3),
                details: vec![i + 20],
                created_at: start + chrono::Duration::milliseconds(i64::from(i) * 30 + 20),
            };
            tag3_note_ids.push(note.header.id());
            db.store_note(&note).await.unwrap();
        }

        let cursor = 0;

        // Fetch all tags, no limit
        let fetched_notes = db.fetch_notes(&[tag1, tag2, tag3], cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 9);
        assert_eq!(fetched_notes[0].header.id(), tag1_note_ids[0]);
        assert_eq!(fetched_notes[1].header.id(), tag2_note_ids[0]);
        assert_eq!(fetched_notes[2].header.id(), tag3_note_ids[0]);
        assert_eq!(fetched_notes[3].header.id(), tag1_note_ids[1]);
        assert_eq!(fetched_notes[4].header.id(), tag2_note_ids[1]);
        assert_eq!(fetched_notes[5].header.id(), tag3_note_ids[1]);
        assert_eq!(fetched_notes[6].header.id(), tag1_note_ids[2]);
        assert_eq!(fetched_notes[7].header.id(), tag2_note_ids[2]);
        assert_eq!(fetched_notes[8].header.id(), tag3_note_ids[2]);

        // Fetch all tags, limit of 5 notes
        let fetched_notes = db.fetch_notes(&[tag1, tag2, tag3], cursor, Some(5)).await.unwrap();
        assert_eq!(fetched_notes.len(), 5);
        assert_eq!(fetched_notes[0].header.id(), tag1_note_ids[0]);
        assert_eq!(fetched_notes[1].header.id(), tag2_note_ids[0]);
        assert_eq!(fetched_notes[2].header.id(), tag3_note_ids[0]);
        assert_eq!(fetched_notes[3].header.id(), tag1_note_ids[1]);
        assert_eq!(fetched_notes[4].header.id(), tag2_note_ids[1]);

        // Fetch only 2 tags, no limit
        let fetched_notes = db.fetch_notes(&[tag1, tag2], cursor, None).await.unwrap();
        assert_eq!(fetched_notes.len(), 6);
        assert_eq!(fetched_notes[0].header.id(), tag1_note_ids[0]);
        assert_eq!(fetched_notes[1].header.id(), tag2_note_ids[0]);
        assert_eq!(fetched_notes[2].header.id(), tag1_note_ids[1]);
        assert_eq!(fetched_notes[3].header.id(), tag2_note_ids[1]);
        assert_eq!(fetched_notes[4].header.id(), tag1_note_ids[2]);
        assert_eq!(fetched_notes[5].header.id(), tag2_note_ids[2]);
    }
}

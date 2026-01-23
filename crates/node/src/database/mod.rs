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
    use crate::test_utils::{TAG_LOCAL_ANY, test_note_header};

    #[tokio::test]
    async fn test_sqlite_database() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();
        let start = Utc::now();

        let note = StoredNote {
            header: test_note_header(),
            details: vec![1, 2, 3, 4],
            created_at: Utc::now(),
        };

        db.store_note(&note).await.unwrap();

        let fetched_notes = db
            .fetch_notes(TAG_LOCAL_ANY.into(), start.timestamp_micros().try_into().unwrap())
            .await
            .unwrap();
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
    async fn test_fetch_notes_timestamp_filtering() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        // Create a note with a specific received_at time
        let received_time = Utc::now();
        let note = StoredNote {
            header: test_note_header(),
            details: vec![1, 2, 3, 4],
            created_at: received_time,
        };

        db.store_note(&note).await.unwrap();

        // Fetch notes with cursor before the note was received - should return the note
        let before_cursor = (received_time - chrono::Duration::seconds(1))
            .timestamp_micros()
            .try_into()
            .unwrap();
        let fetched_notes = db.fetch_notes(TAG_LOCAL_ANY.into(), before_cursor).await.unwrap();
        assert_eq!(fetched_notes.len(), 1);
        assert_eq!(fetched_notes[0].header.id(), note.header.id());

        // Fetch notes with cursor after the note was received - should return empty
        let after_cursor = (received_time + chrono::Duration::seconds(1))
            .timestamp_micros()
            .try_into()
            .unwrap();
        let fetched_notes = db.fetch_notes(TAG_LOCAL_ANY.into(), after_cursor).await.unwrap();
        assert_eq!(fetched_notes.len(), 0);
    }
}

use std::str::FromStr;

use chrono::{DateTime, Utc};
use miden_protocol::note::NoteHeader;
use miden_protocol::utils::serde::{Deserializable, Serializable};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use super::{DatabaseBackend, DatabaseError};
use crate::metrics::MetricsDatabase;
use crate::types::{NoteId, NoteTag, StoredNote};

pub(crate) const FETCH_NOTES_BATCH_SIZE: i64 = 500;
const LEGACY_CURSOR_THRESHOLD: u64 = 1_000_000_000_000;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/database/sqlite/migrations");

pub struct SqliteDatabase {
    pool: SqlitePool,
    metrics: MetricsDatabase,
}

impl SqliteDatabase {
    pub async fn connect(url: &str, metrics: MetricsDatabase) -> Result<Self, DatabaseError> {
        let normalized = normalize_url(url);
        let is_memory = normalized == "sqlite::memory:";
        let options = SqliteConnectOptions::from_str(&normalized)
            .map_err(|error| DatabaseError::Configuration(error.to_string()))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(std::time::Duration::from_secs(30));
        let pool = SqlitePoolOptions::new()
            .max_connections(if is_memory { 1 } else { 16 })
            .connect_with(options)
            .await
            .map_err(connection_error)?;
        verify_supported_schema(&pool).await?;
        MIGRATOR.run(&pool).await.map_err(migration_error)?;
        Ok(Self { pool, metrics })
    }
}

#[async_trait::async_trait]
impl DatabaseBackend for SqliteDatabase {
    #[tracing::instrument(skip(self, note), fields(operation = "db.store_note"))]
    async fn store_note(&self, note: &StoredNote) -> Result<(), DatabaseError> {
        let timer = self.metrics.db_store_note();
        sqlx::query(
            "INSERT INTO notes (id, tag, header, details, created_at, after_block_num) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(note.header.id().as_bytes().as_slice())
        .bind(i64::from(note.header.metadata().tag().as_u32()))
        .bind(note.header.to_bytes())
        .bind(&note.details)
        .bind(note.created_at.timestamp_micros())
        .bind(note.after_block_num.map(i64::from))
        .execute(&self.pool)
        .await
        .map_err(query_error)?;
        timer.finish("ok");
        Ok(())
    }

    #[tracing::instrument(skip(self, tags), fields(
        operation = "db.fetch_notes_by_tags",
        tag_count = tags.len(),
        cursor,
        notes_returned = tracing::field::Empty,
    ))]
    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();
        if tags.is_empty() {
            timer.finish("ok");
            return Ok(Vec::new());
        }

        let effective_cursor = if cursor > LEGACY_CURSOR_THRESHOLD {
            self.metrics.db_fetch_notes_legacy_cursor_reset();
            tracing::info!(original_cursor = cursor, "Legacy cursor reset to 0");
            0
        } else {
            cursor
        };
        let cursor = i64::try_from(effective_cursor).map_err(|_| {
            DatabaseError::QueryExecution("cursor exceeds SQLite range".to_string())
        })?;

        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT seq, header, details, created_at, after_block_num \
             FROM notes WHERE seq > ",
        );
        query.push_bind(cursor).push(" AND tag IN (");
        let mut separated = query.separated(", ");
        for tag in tags {
            separated.push_bind(i64::from(tag.as_u32()));
        }
        separated.push_unseparated(") ORDER BY seq LIMIT ");
        query.push_bind(FETCH_NOTES_BATCH_SIZE);

        let rows = query.build().fetch_all(&self.pool).await.map_err(query_error)?;
        let notes: Result<Vec<_>, _> = rows.iter().map(row_to_note).collect();
        let notes = notes?;
        tracing::Span::current().record("notes_returned", notes.len());
        timer.finish("ok");
        Ok(notes)
    }

    async fn cleanup_old_notes(&self, retention_days: u32) -> Result<u64, DatabaseError> {
        let cutoff =
            (Utc::now() - chrono::Duration::days(i64::from(retention_days))).timestamp_micros();
        let result = sqlx::query("DELETE FROM notes WHERE created_at < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(query_error)?;
        Ok(result.rows_affected())
    }

    async fn note_exists(&self, note_id: NoteId) -> Result<bool, DatabaseError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?)")
            .bind(note_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(query_error)
    }
}

fn normalize_url(url: &str) -> String {
    if url == ":memory:" {
        "sqlite::memory:".to_string()
    } else if url.starts_with("sqlite:") {
        url.to_string()
    } else {
        format!("sqlite://{url}")
    }
}

async fn verify_supported_schema(pool: &SqlitePool) -> Result<(), DatabaseError> {
    let has_notes: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'notes')",
    )
    .fetch_one(pool)
    .await
    .map_err(query_error)?;
    if !has_notes {
        return Ok(());
    }

    let columns = sqlx::query("PRAGMA table_info(notes)")
        .fetch_all(pool)
        .await
        .map_err(query_error)?;
    let has_column = |name: &str| {
        columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").is_ok_and(|column| column == name))
    };
    let required = ["seq", "id", "tag", "header", "details", "created_at", "after_block_num"];
    if required.into_iter().all(has_column) {
        Ok(())
    } else {
        Err(DatabaseError::Migration(
            "unsupported legacy SQLite schema; upgrade it with the previous release first"
                .to_string(),
        ))
    }
}

fn row_to_note(row: &SqliteRow) -> Result<StoredNote, DatabaseError> {
    let header_bytes: Vec<u8> = row.try_get("header").map_err(query_error)?;
    let header = NoteHeader::read_from_bytes(&header_bytes)
        .map_err(|error| DatabaseError::Deserialization(error.to_string()))?;
    let timestamp: i64 = row.try_get("created_at").map_err(query_error)?;
    let created_at = DateTime::from_timestamp_micros(timestamp)
        .ok_or_else(|| DatabaseError::Deserialization(format!("invalid timestamp: {timestamp}")))?;
    let after_block_num: Option<i64> = row.try_get("after_block_num").map_err(query_error)?;
    Ok(StoredNote {
        header,
        details: row.try_get("details").map_err(query_error)?,
        created_at,
        seq: row.try_get("seq").map_err(query_error)?,
        after_block_num: after_block_num
            .map(|value| {
                u32::try_from(value).map_err(|_| {
                    DatabaseError::Deserialization(format!("invalid block number: {value}"))
                })
            })
            .transpose()?,
    })
}

fn connection_error(error: sqlx::Error) -> DatabaseError {
    let message = error.to_string();
    drop(error);
    DatabaseError::Connection(message)
}

fn query_error(error: sqlx::Error) -> DatabaseError {
    let message = error.to_string();
    drop(error);
    DatabaseError::QueryExecution(message)
}

fn migration_error(error: sqlx::migrate::MigrateError) -> DatabaseError {
    let message = error.to_string();
    drop(error);
    DatabaseError::Migration(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_unsupported_legacy_schema_before_serving() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE notes (\
             id BLOB PRIMARY KEY, tag INTEGER NOT NULL, header BLOB NOT NULL, \
             details BLOB NOT NULL, created_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = verify_supported_schema(&pool).await.unwrap_err();
        assert!(matches!(error, DatabaseError::Migration(_)));
    }
}

use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use miden_protocol::note::NoteHeader;
use miden_protocol::utils::serde::{Deserializable, Serializable};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use super::{
    DatabaseBackend,
    DatabaseError,
    DatabaseNotifications,
    DatabaseWatch,
    FetchPage,
    StoreResult,
};
use crate::metrics::MetricsDatabase;
use crate::types::{NoteTag, StoredNote};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/database/sqlite/migrations");

pub struct SqliteDatabase {
    pool: SqlitePool,
    notifications: DatabaseNotifications,
    metrics: MetricsDatabase,
}

impl SqliteDatabase {
    pub async fn connect(
        url: &str,
        allow_in_memory: bool,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError> {
        let pool = open_pool(url, allow_in_memory, false).await?;
        verify_schema(&pool).await?;
        let retained: i64 =
            sqlx::query_scalar("SELECT retained_bytes FROM storage_metadata WHERE singleton = 1")
                .fetch_one(&pool)
                .await
                .map_err(query_error)?;
        metrics.record_retained_bytes(u64::try_from(retained).unwrap_or(0));
        Ok(Self {
            pool,
            notifications: DatabaseNotifications::new(),
            metrics,
        })
    }

    #[cfg(any(test, feature = "testing"))]
    pub async fn connect_and_migrate_for_test(
        url: &str,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError> {
        let pool = open_pool(url, true, true).await?;
        MIGRATOR.run(&pool).await.map_err(migration_error)?;
        Ok(Self {
            pool,
            notifications: DatabaseNotifications::new(),
            metrics,
        })
    }

    pub async fn migrate(url: &str, allow_in_memory: bool) -> Result<(), DatabaseError> {
        let pool = open_pool(url, allow_in_memory, true).await?;
        MIGRATOR.run(&pool).await.map_err(migration_error)
    }
}

#[async_trait::async_trait]
impl DatabaseBackend for SqliteDatabase {
    #[tracing::instrument(skip(self, note), fields(operation = "db.store_note"))]
    async fn store_note(
        &self,
        note: &StoredNote,
        max_retained_bytes: u64,
    ) -> Result<StoreResult, DatabaseError> {
        let timer = self.metrics.db_store_note();
        let note_bytes = note.header.to_bytes().len() + note.details.len();
        if note_bytes > super::FETCH_NOTES_MAX_BYTES {
            return Err(DatabaseError::Capacity(format!(
                "note exceeds the {} byte fetch limit",
                super::FETCH_NOTES_MAX_BYTES
            )));
        }
        let note_id = note.header.id();
        let mut tx = self.pool.begin().await.map_err(query_error)?;

        sqlx::query("UPDATE storage_metadata SET next_cursor = next_cursor WHERE singleton = 1")
            .execute(&mut *tx)
            .await
            .map_err(query_error)?;

        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notes WHERE id = ?)")
            .bind(note_id.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(query_error)?;
        let current_retained: i64 =
            sqlx::query_scalar("SELECT retained_bytes FROM storage_metadata WHERE singleton = 1")
                .fetch_one(&mut *tx)
                .await
                .map_err(query_error)?;
        if exists {
            tx.rollback().await.map_err(query_error)?;
            self.metrics.record_retained_bytes(u64::try_from(current_retained).unwrap_or(0));
            timer.finish("ok");
            return Ok(StoreResult::AlreadyPresent);
        }

        let retained_bytes = i64::try_from(note_bytes)
            .map_err(|_| DatabaseError::Serialization("note is too large".to_string()))?;
        let next_retained = current_retained
            .checked_add(retained_bytes)
            .ok_or_else(|| DatabaseError::Capacity("retained byte count overflow".to_string()))?;
        if u64::try_from(next_retained).unwrap_or(u64::MAX) > max_retained_bytes {
            return Err(DatabaseError::Capacity(format!(
                "accepting this note would exceed the {max_retained_bytes} byte limit"
            )));
        }
        let seq: i64 = sqlx::query_scalar(
            "UPDATE storage_metadata \
             SET next_cursor = next_cursor + 1, retained_bytes = retained_bytes + ? \
             WHERE singleton = 1 RETURNING next_cursor - 1",
        )
        .bind(retained_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;

        sqlx::query(
            "INSERT INTO notes \
             (seq, id, tag, header, details, created_at, after_block_num) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(seq)
        .bind(note_id.as_bytes().as_slice())
        .bind(i64::from(note.header.metadata().tag().as_u32()))
        .bind(note.header.to_bytes())
        .bind(&note.details)
        .bind(note.created_at.timestamp_micros())
        .bind(note.after_block_num.map(i64::from))
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;

        tx.commit().await.map_err(query_error)?;
        self.notifications.notify(note.header.metadata().tag());
        self.metrics
            .record_retained_bytes(u64::try_from(next_retained).unwrap_or(u64::MAX));
        timer.finish("ok");
        Ok(StoreResult::Inserted)
    }

    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
        max_rows: u32,
        max_bytes: usize,
    ) -> Result<FetchPage, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();
        if tags.is_empty() {
            timer.finish("ok");
            return Ok(FetchPage { notes: Vec::new(), has_more: false });
        }
        let cursor = i64::try_from(cursor).map_err(|_| {
            DatabaseError::QueryExecution("cursor exceeds SQLite range".to_string())
        })?;
        let max_bytes = i64::try_from(max_bytes)
            .map_err(|_| DatabaseError::QueryExecution("byte limit is too large".to_string()))?;

        let candidate_limit = i64::from(max_rows) + 1;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT seq, header, details, created_at, after_block_num, candidate_count FROM (\
             SELECT seq, header, details, created_at, after_block_num, \
             SUM(LENGTH(header) + LENGTH(details)) OVER (ORDER BY seq) AS running_bytes, \
             COUNT(*) OVER () AS candidate_count FROM (\
             SELECT seq, header, details, created_at, after_block_num \
             FROM notes WHERE seq > ",
        );
        query.push_bind(cursor).push(" AND tag IN (");
        let mut separated = query.separated(", ");
        for tag in tags {
            separated.push_bind(i64::from(tag.as_u32()));
        }
        separated.push_unseparated(") ORDER BY seq LIMIT ");
        query
            .push_bind(candidate_limit)
            .push(") AS candidates) AS bounded WHERE running_bytes <= ")
            .push_bind(max_bytes)
            .push(" ORDER BY seq LIMIT ")
            .push_bind(i64::from(max_rows));

        let rows = query.build().fetch_all(&self.pool).await.map_err(query_error)?;
        let candidate_count = rows
            .first()
            .map_or(Ok(0_i64), |row| row.try_get("candidate_count"))
            .map_err(query_error)?;
        let notes: Result<Vec<_>, _> = rows.iter().map(row_to_note).collect();
        let notes = notes?;
        let has_more = candidate_count > i64::try_from(notes.len()).unwrap_or(i64::MAX);
        timer.finish("ok");
        Ok(FetchPage { notes, has_more })
    }

    async fn cleanup_old_notes(
        &self,
        retention_days: u32,
        max_rows: u32,
    ) -> Result<u64, DatabaseError> {
        let cutoff =
            (Utc::now() - chrono::Duration::days(i64::from(retention_days))).timestamp_micros();
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        sqlx::query("UPDATE storage_metadata SET next_cursor = next_cursor WHERE singleton = 1")
            .execute(&mut *tx)
            .await
            .map_err(query_error)?;
        let rows = sqlx::query(
            "SELECT seq, LENGTH(header) + LENGTH(details) AS retained_bytes \
             FROM notes WHERE created_at < ? ORDER BY seq LIMIT ?",
        )
        .bind(cutoff)
        .bind(i64::from(max_rows))
        .fetch_all(&mut *tx)
        .await
        .map_err(query_error)?;
        if rows.is_empty() {
            tx.rollback().await.map_err(query_error)?;
            return Ok(0);
        }
        let seqs: Vec<i64> = rows.iter().map(|row| row.get("seq")).collect();
        let removed_bytes: i64 = rows.iter().map(|row| row.get::<i64, _>("retained_bytes")).sum();
        let mut delete = QueryBuilder::<Sqlite>::new("DELETE FROM notes WHERE seq IN (");
        let mut separated = delete.separated(", ");
        for seq in &seqs {
            separated.push_bind(seq);
        }
        separated.push_unseparated(")");
        delete.build().execute(&mut *tx).await.map_err(query_error)?;
        let retained: i64 = sqlx::query_scalar(
            "UPDATE storage_metadata SET retained_bytes = retained_bytes - ? \
             WHERE singleton = 1 RETURNING retained_bytes",
        )
        .bind(removed_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
        self.metrics.record_retained_bytes(u64::try_from(retained).unwrap_or(0));
        Ok(seqs.len() as u64)
    }

    fn subscribe(&self, tag: NoteTag) -> tokio::sync::watch::Receiver<DatabaseWatch> {
        self.notifications.subscribe(tag)
    }

    async fn is_ready(&self) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&self.pool).await.is_ok()
    }
}

async fn open_pool(
    url: &str,
    allow_in_memory: bool,
    create_if_missing: bool,
) -> Result<SqlitePool, DatabaseError> {
    let is_memory = url == "sqlite::memory:";
    if is_memory && !allow_in_memory {
        return Err(DatabaseError::Configuration(
            "in-memory SQLite is only available to tests".to_string(),
        ));
    }
    if !is_memory {
        let path = sqlite_path(url)?;
        if !create_if_missing && !path.exists() {
            return Err(DatabaseError::Configuration(format!(
                "SQLite database does not exist: {}. Run the migrate command first",
                path.display()
            )));
        }
    }
    let normalized = normalize_url(url);
    let options = SqliteConnectOptions::from_str(&normalized)
        .map_err(|error| DatabaseError::Configuration(error.to_string()))?
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
        .busy_timeout(std::time::Duration::from_secs(30));
    SqlitePoolOptions::new()
        .max_connections(if is_memory { 1 } else { 16 })
        .connect_with(options)
        .await
        .map_err(connection_error)
}

fn normalize_url(url: &str) -> String {
    if url.starts_with("sqlite:") {
        url.to_string()
    } else {
        format!("sqlite://{url}")
    }
}

fn sqlite_path(url: &str) -> Result<&Path, DatabaseError> {
    let path = url.strip_prefix("sqlite://").unwrap_or(url);
    if path.is_empty() {
        return Err(DatabaseError::Configuration("SQLite path is empty".to_string()));
    }
    Ok(Path::new(path))
}

async fn verify_schema(pool: &SqlitePool) -> Result<(), DatabaseError> {
    let applied = sqlx::query(
        "SELECT version, checksum FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| {
        DatabaseError::Migration("database is not migrated; run the migrate command".to_string())
    })?;
    let expected: Vec<_> = MIGRATOR.iter().collect();
    let current = applied.len() == expected.len()
        && applied.iter().zip(expected).all(|(row, migration)| {
            let version: Result<i64, _> = row.try_get("version");
            let checksum: Result<Vec<u8>, _> = row.try_get("checksum");
            version.is_ok_and(|value| value == migration.version)
                && checksum.is_ok_and(|value| value == migration.checksum.as_ref())
        });
    if !current {
        return Err(DatabaseError::Migration(
            "database schema is not current; run the migrate command".to_string(),
        ));
    }
    Ok(())
}

fn row_to_note(row: &SqliteRow) -> Result<StoredNote, DatabaseError> {
    let header_bytes: Vec<u8> = row.try_get("header").map_err(query_error)?;
    let header = NoteHeader::read_from_bytes(&header_bytes)
        .map_err(|error| DatabaseError::Deserialization(error.to_string()))?;
    let created_at_micros: i64 = row.try_get("created_at").map_err(query_error)?;
    let created_at = DateTime::from_timestamp_micros(created_at_micros).ok_or_else(|| {
        DatabaseError::Deserialization(format!("invalid timestamp: {created_at_micros}"))
    })?;
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
    async fn serving_rejects_a_migration_checksum_mismatch() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let url = file.path().to_string_lossy();
        SqliteDatabase::migrate(&url, false).await.unwrap();
        let pool = open_pool(&url, false, false).await.unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00'")
            .execute(&pool)
            .await
            .unwrap();

        let error = verify_schema(&pool)
            .await
            .expect_err("a changed migration must not be accepted");
        assert!(matches!(error, DatabaseError::Migration(_)));
    }
}

use chrono::{DateTime, Utc};
use miden_protocol::note::NoteHeader;
use miden_protocol::utils::serde::{Deserializable, Serializable};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

use super::sqlite::SqliteDatabase;
use super::{DatabaseBackend, DatabaseError, StorageMetadata, StoreResult, envelope_digest};
use crate::metrics::MetricsDatabase;
use crate::types::{NoteTag, StoredNote};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/database/postgres/migrations");

pub struct PostgresDatabase {
    pool: PgPool,
    metrics: MetricsDatabase,
}

impl PostgresDatabase {
    pub async fn connect(url: &str, metrics: MetricsDatabase) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .connect(url)
            .await
            .map_err(connection_error)?;
        verify_schema(&pool).await?;
        let retained: i64 = sqlx::query_scalar(
            "SELECT retained_bytes FROM storage_metadata WHERE singleton = TRUE",
        )
        .fetch_one(&pool)
        .await
        .map_err(query_error)?;
        metrics.record_retained_bytes(u64::try_from(retained).unwrap_or(0));

        Ok(Self { pool, metrics })
    }

    pub async fn migrate(url: &str) -> Result<(), DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(connection_error)?;
        MIGRATOR.run(&pool).await.map_err(migration_error)
    }

    pub async fn import_from_sqlite(
        &self,
        source: &SqliteDatabase,
        expected: &StorageMetadata,
    ) -> Result<u64, DatabaseError> {
        if expected.row_count < 0 || expected.next_cursor < 1 || expected.retained_bytes < 0 {
            return Err(DatabaseError::Migration(
                "SQLite storage metadata contains a negative value".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        let metadata = sqlx::query(
            "SELECT next_cursor, retained_bytes FROM storage_metadata \
             WHERE singleton = TRUE FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;
        let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
            .fetch_one(&mut *tx)
            .await
            .map_err(query_error)?;
        let existing_next: i64 = metadata.try_get("next_cursor").map_err(query_error)?;
        let existing_retained: i64 = metadata.try_get("retained_bytes").map_err(query_error)?;
        if existing != 0 || existing_next != 1 || existing_retained != 0 {
            return Err(DatabaseError::Configuration(
                "PostgreSQL destination must be empty".to_string(),
            ));
        }

        let mut cursor = 0_i64;
        let mut row_count = 0_i64;
        let mut retained_bytes = 0_i64;
        let mut source_digest = copy_hasher();
        loop {
            let notes = source.export_batch(cursor).await?;
            if notes.is_empty() {
                break;
            }
            for note in notes {
                if note.seq <= cursor {
                    return Err(DatabaseError::Migration(
                        "SQLite cursors are not strictly increasing".to_string(),
                    ));
                }
                let envelope_bytes = insert_copy_note(&mut tx, &note, &mut source_digest).await?;
                retained_bytes = retained_bytes
                    .checked_add(i64::try_from(envelope_bytes).map_err(|_| {
                        DatabaseError::Serialization("note is too large".to_string())
                    })?)
                    .ok_or_else(|| {
                        DatabaseError::Serialization("retained byte count overflow".to_string())
                    })?;
                row_count = row_count.checked_add(1).ok_or_else(|| {
                    DatabaseError::Serialization("row count overflow".to_string())
                })?;
                cursor = note.seq;
            }
        }
        let minimum_next_cursor = cursor.checked_add(1).ok_or_else(|| {
            DatabaseError::Serialization("cursor overflow during copy".to_string())
        })?;
        if row_count != expected.row_count
            || retained_bytes != expected.retained_bytes
            || minimum_next_cursor > expected.next_cursor
        {
            return Err(DatabaseError::Migration(
                "SQLite storage metadata does not match its notes".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE storage_metadata SET next_cursor = $1, retained_bytes = $2 \
             WHERE singleton = TRUE",
        )
        .bind(expected.next_cursor)
        .bind(expected.retained_bytes)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
        verify_copy(&mut tx, expected, source_digest.finalize()).await?;
        tx.commit().await.map_err(query_error)?;
        self.metrics
            .record_retained_bytes(u64::try_from(expected.retained_bytes).unwrap_or(u64::MAX));
        u64::try_from(row_count)
            .map_err(|_| DatabaseError::Serialization("row count overflow".to_string()))
    }
}

fn copy_hasher() -> blake3::Hasher {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"miden-note-transport-copy-v1");
    hasher
}

fn hash_copy_entry(hasher: &mut blake3::Hasher, seq: i64, digest: &[u8]) {
    hasher.update(&seq.to_le_bytes());
    hasher.update(digest);
}

async fn insert_copy_note(
    tx: &mut Transaction<'_, Postgres>,
    note: &StoredNote,
    copy_digest: &mut blake3::Hasher,
) -> Result<usize, DatabaseError> {
    let envelope_bytes = note.header.to_bytes().len() + note.details.len();
    if envelope_bytes > super::FETCH_NOTES_MAX_BYTES {
        return Err(DatabaseError::Migration(format!(
            "note at cursor {} exceeds the V1 envelope limit",
            note.seq
        )));
    }
    let digest = envelope_digest(note);
    hash_copy_entry(copy_digest, note.seq, &digest);
    sqlx::query(
        "INSERT INTO notes \
         (seq, envelope_digest, id, tag, header, details, created_at, after_block_num) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(note.seq)
    .bind(digest.as_slice())
    .bind(note.header.id().as_bytes().as_slice())
    .bind(i64::from(note.header.metadata().tag().as_u32()))
    .bind(note.header.to_bytes())
    .bind(&note.details)
    .bind(note.created_at.timestamp_micros())
    .bind(note.after_block_num.map(i64::from))
    .execute(&mut **tx)
    .await
    .map_err(query_error)?;
    Ok(envelope_bytes)
}

async fn verify_copy(
    tx: &mut Transaction<'_, Postgres>,
    expected: &StorageMetadata,
    source_digest: blake3::Hash,
) -> Result<(), DatabaseError> {
    let totals = sqlx::query(
        "SELECT COUNT(*) AS row_count, \
         COALESCE(SUM(OCTET_LENGTH(header)::BIGINT + OCTET_LENGTH(details)::BIGINT), 0)::BIGINT \
             AS retained_bytes \
         FROM notes",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(query_error)?;
    let actual_count: i64 = totals.try_get("row_count").map_err(query_error)?;
    let actual_retained: i64 = totals.try_get("retained_bytes").map_err(query_error)?;
    if actual_count != expected.row_count || actual_retained != expected.retained_bytes {
        return Err(DatabaseError::Migration(
            "PostgreSQL row count or retained bytes failed verification".to_string(),
        ));
    }

    let mut cursor = 0_i64;
    let mut destination_digest = copy_hasher();
    loop {
        let rows = sqlx::query(
            "SELECT seq, envelope_digest FROM notes WHERE seq > $1 ORDER BY seq LIMIT $2",
        )
        .bind(cursor)
        .bind(i64::from(super::FETCH_NOTES_MAX_ROWS))
        .fetch_all(&mut **tx)
        .await
        .map_err(query_error)?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let seq: i64 = row.try_get("seq").map_err(query_error)?;
            let digest: Vec<u8> = row.try_get("envelope_digest").map_err(query_error)?;
            hash_copy_entry(&mut destination_digest, seq, &digest);
            cursor = seq;
        }
    }
    if source_digest != destination_digest.finalize() {
        return Err(DatabaseError::Migration(
            "PostgreSQL envelope digests failed verification".to_string(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl DatabaseBackend for PostgresDatabase {
    #[tracing::instrument(skip(self, note), fields(operation = "db.store_note"))]
    async fn store_note(
        &self,
        note: &StoredNote,
        max_retained_bytes: u64,
    ) -> Result<StoreResult, DatabaseError> {
        let timer = self.metrics.db_store_note();
        let envelope_bytes = note.header.to_bytes().len() + note.details.len();
        if envelope_bytes > super::FETCH_NOTES_MAX_BYTES {
            return Err(DatabaseError::Capacity(format!(
                "envelope exceeds the {} byte fetch limit",
                super::FETCH_NOTES_MAX_BYTES
            )));
        }
        let digest = envelope_digest(note);
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        let metadata = sqlx::query(
            "SELECT next_cursor, retained_bytes FROM storage_metadata \
             WHERE singleton = TRUE FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;
        let seq: i64 = metadata.try_get("next_cursor").map_err(query_error)?;
        let current_retained: i64 = metadata.try_get("retained_bytes").map_err(query_error)?;

        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notes WHERE envelope_digest = $1)")
                .bind(digest.as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(query_error)?;
        if exists {
            tx.rollback().await.map_err(query_error)?;
            self.metrics.record_retained_bytes(u64::try_from(current_retained).unwrap_or(0));
            timer.finish("ok");
            return Ok(StoreResult::AlreadyPresent);
        }

        let retained_bytes = i64::try_from(envelope_bytes)
            .map_err(|_| DatabaseError::Serialization("note is too large".to_string()))?;
        let next_retained = current_retained
            .checked_add(retained_bytes)
            .ok_or_else(|| DatabaseError::Capacity("retained byte count overflow".to_string()))?;
        if u64::try_from(next_retained).unwrap_or(u64::MAX) > max_retained_bytes {
            return Err(DatabaseError::Capacity(format!(
                "accepting this envelope would exceed the {max_retained_bytes} byte limit"
            )));
        }
        sqlx::query(
            "UPDATE storage_metadata SET next_cursor = next_cursor + 1, \
             retained_bytes = retained_bytes + $1 WHERE singleton = TRUE",
        )
        .bind(retained_bytes)
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
        sqlx::query(
            "INSERT INTO notes \
             (seq, envelope_digest, id, tag, header, details, created_at, after_block_num) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(seq)
        .bind(digest.as_slice())
        .bind(note.header.id().as_bytes().as_slice())
        .bind(i64::from(note.header.metadata().tag().as_u32()))
        .bind(note.header.to_bytes())
        .bind(&note.details)
        .bind(note.created_at.timestamp_micros())
        .bind(note.after_block_num.map(i64::from))
        .execute(&mut *tx)
        .await
        .map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
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
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();
        if tags.is_empty() {
            timer.finish("ok");
            return Ok(Vec::new());
        }
        let cursor = i64::try_from(cursor).map_err(|_| {
            DatabaseError::QueryExecution("cursor exceeds PostgreSQL range".to_string())
        })?;
        let max_bytes = i64::try_from(max_bytes)
            .map_err(|_| DatabaseError::QueryExecution("byte limit is too large".to_string()))?;

        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT seq, header, details, created_at, after_block_num FROM (\
             SELECT seq, header, details, created_at, after_block_num, \
             SUM(OCTET_LENGTH(header) + OCTET_LENGTH(details)) OVER (ORDER BY seq) AS running_bytes \
             FROM notes WHERE seq > ",
        );
        query.push_bind(cursor).push(" AND tag IN (");
        let mut separated = query.separated(", ");
        for tag in tags {
            separated.push_bind(i64::from(tag.as_u32()));
        }
        separated.push_unseparated(") ");
        query
            .push("ORDER BY seq) AS bounded WHERE running_bytes <= ")
            .push_bind(max_bytes)
            .push(" ORDER BY seq LIMIT ")
            .push_bind(i64::from(max_rows));
        let rows = query.build().fetch_all(&self.pool).await.map_err(query_error)?;
        let notes = rows.iter().map(row_to_note).collect();
        timer.finish("ok");
        notes
    }

    async fn cleanup_old_notes(
        &self,
        retention_days: u32,
        max_rows: u32,
    ) -> Result<u64, DatabaseError> {
        let cutoff =
            (Utc::now() - chrono::Duration::days(i64::from(retention_days))).timestamp_micros();
        let mut tx = self.pool.begin().await.map_err(query_error)?;
        sqlx::query("SELECT next_cursor FROM storage_metadata WHERE singleton = TRUE FOR UPDATE")
            .execute(&mut *tx)
            .await
            .map_err(query_error)?;
        let rows = sqlx::query(
            "SELECT seq, OCTET_LENGTH(header)::BIGINT + OCTET_LENGTH(details)::BIGINT \
                 AS retained_bytes \
             FROM notes WHERE created_at < $1 ORDER BY seq LIMIT $2",
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
        let mut delete = QueryBuilder::<Postgres>::new("DELETE FROM notes WHERE seq IN (");
        let mut separated = delete.separated(", ");
        for seq in &seqs {
            separated.push_bind(seq);
        }
        separated.push_unseparated(")");
        delete.build().execute(&mut *tx).await.map_err(query_error)?;
        let retained: i64 = sqlx::query_scalar(
            "UPDATE storage_metadata SET retained_bytes = retained_bytes - $1 \
             WHERE singleton = TRUE RETURNING retained_bytes",
        )
        .bind(removed_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(query_error)?;
        tx.commit().await.map_err(query_error)?;
        self.metrics.record_retained_bytes(u64::try_from(retained).unwrap_or(0));
        Ok(seqs.len() as u64)
    }
}

async fn verify_schema(pool: &PgPool) -> Result<(), DatabaseError> {
    let applied = sqlx::query(
        "SELECT version, checksum FROM _sqlx_migrations WHERE success = TRUE ORDER BY version",
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

fn row_to_note(row: &PgRow) -> Result<StoredNote, DatabaseError> {
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

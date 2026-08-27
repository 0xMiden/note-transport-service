use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use miden_protocol::note::NoteHeader;
use miden_protocol::utils::serde::{Deserializable, Serializable};
use sqlx::postgres::{PgListener, PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use tokio::sync::watch;

use super::{
    DatabaseBackend,
    DatabaseError,
    DatabaseWatch,
    FetchPage,
    StoreResult,
    envelope_digest,
};
use crate::metrics::MetricsDatabase;
use crate::types::{NoteTag, StoredNote};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("src/database/postgres/migrations");
const CHANGE_CHANNEL: &str = "miden_note_transport_changes";

pub struct PostgresDatabase {
    pool: PgPool,
    changes: watch::Sender<DatabaseWatch>,
    listener_ready: Arc<AtomicBool>,
    listener: tokio::task::JoinHandle<()>,
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

        let mut pg_listener = PgListener::connect(url).await.map_err(connection_error)?;
        pg_listener.listen(CHANGE_CHANNEL).await.map_err(connection_error)?;
        pg_listener.eager_reconnect(false);
        let (changes, _) = watch::channel(DatabaseWatch::ready());
        let listener_ready = Arc::new(AtomicBool::new(true));
        let listener =
            spawn_listener(url.to_string(), pg_listener, changes.clone(), listener_ready.clone());

        Ok(Self {
            pool,
            changes,
            listener_ready,
            listener,
            metrics,
        })
    }

    pub async fn migrate(url: &str) -> Result<(), DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await
            .map_err(connection_error)?;
        MIGRATOR.run(&pool).await.map_err(migration_error)
    }
}

impl Drop for PostgresDatabase {
    fn drop(&mut self) {
        self.listener.abort();
    }
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
        sqlx::query("SELECT pg_notify($1, '')")
            .bind(CHANGE_CHANNEL)
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
    ) -> Result<FetchPage, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();
        if tags.is_empty() {
            timer.finish("ok");
            return Ok(FetchPage { notes: Vec::new(), has_more: false });
        }
        let cursor = i64::try_from(cursor).map_err(|_| {
            DatabaseError::QueryExecution("cursor exceeds PostgreSQL range".to_string())
        })?;
        let max_bytes = i64::try_from(max_bytes)
            .map_err(|_| DatabaseError::QueryExecution("byte limit is too large".to_string()))?;

        let candidate_limit = i64::from(max_rows) + 1;
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT seq, header, details, created_at, after_block_num, candidate_count FROM (\
             SELECT seq, header, details, created_at, after_block_num, \
             SUM(OCTET_LENGTH(header) + OCTET_LENGTH(details)) OVER (ORDER BY seq) AS running_bytes, \
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

    fn subscribe(&self) -> watch::Receiver<DatabaseWatch> {
        self.changes.subscribe()
    }

    async fn is_ready(&self) -> bool {
        self.listener_ready.load(Ordering::Acquire)
            && sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&self.pool).await.is_ok()
    }
}

fn spawn_listener(
    url: String,
    mut listener: PgListener,
    changes: watch::Sender<DatabaseWatch>,
    ready: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match listener.try_recv().await {
                Ok(Some(_)) => changes.send_modify(DatabaseWatch::advance),
                result => {
                    ready.store(false, Ordering::Release);
                    changes.send_modify(|state| {
                        state.ready = false;
                        state.advance();
                    });
                    match result {
                        Ok(None) => {
                            tracing::error!("PostgreSQL note notification listener disconnected");
                        },
                        Err(error) => tracing::error!(
                            %error,
                            "PostgreSQL note notification listener failed"
                        ),
                        Ok(Some(_)) => unreachable!(),
                    }
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        match PgListener::connect(&url).await {
                            Ok(mut replacement) => match replacement.listen(CHANGE_CHANNEL).await {
                                Ok(()) => {
                                    replacement.eager_reconnect(false);
                                    listener = replacement;
                                    ready.store(true, Ordering::Release);
                                    changes.send_modify(|state| {
                                        state.ready = true;
                                        state.advance();
                                    });
                                    break;
                                },
                                Err(error) => tracing::warn!(%error, "PostgreSQL LISTEN failed"),
                            },
                            Err(error) => {
                                tracing::warn!(%error, "PostgreSQL listener reconnect failed");
                            },
                        }
                    }
                },
            }
        }
    })
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use sqlx::postgres::PgConnectOptions;

    use super::*;

    #[tokio::test]
    async fn listener_reports_loss_and_recovery() {
        let Ok(url) = std::env::var("MNT_TEST_POSTGRES_URL") else {
            return;
        };
        let _guard = super::super::POSTGRES_TEST_LOCK.lock().await;
        let pool = PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let application_name = format!("mnt-listener-test-{}", std::process::id());
        let options = PgConnectOptions::from_str(&url).unwrap().application_name(&application_name);
        let listener_pool =
            PgPoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        let mut listener = PgListener::connect_with(&listener_pool).await.unwrap();
        listener.listen(CHANGE_CHANNEL).await.unwrap();
        listener.eager_reconnect(false);
        let pid: i32 =
            sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE application_name = $1")
                .bind(&application_name)
                .fetch_one(&pool)
                .await
                .unwrap();

        let (changes, mut receiver) = watch::channel(DatabaseWatch::ready());
        let ready = Arc::new(AtomicBool::new(true));
        let task = spawn_listener(url, listener, changes, ready.clone());
        let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(terminated);

        tokio::time::timeout(std::time::Duration::from_secs(2), receiver.changed())
            .await
            .expect("listener loss was not reported")
            .unwrap();
        assert!(!receiver.borrow_and_update().is_ready());
        assert!(!ready.load(Ordering::Acquire));

        tokio::time::timeout(std::time::Duration::from_secs(3), receiver.changed())
            .await
            .expect("listener did not reconnect")
            .unwrap();
        assert!(receiver.borrow_and_update().is_ready());
        assert!(ready.load(Ordering::Acquire));
        task.abort();
    }
}

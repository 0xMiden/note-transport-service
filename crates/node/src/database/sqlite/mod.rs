use chrono::Utc;
use diesel::prelude::*;

use crate::database::{DatabaseBackend, DatabaseConfig, DatabaseError};
use crate::metrics::MetricsDatabase;
use crate::types::{NoteId, NoteTag, StoredNote};

mod connection_manager;
mod migrations;
mod models;
mod schema;

use connection_manager::ConnectionManager;
use models::{NewNote, Note};

/// `SQLite` implementation of the database backend
pub struct SqliteDatabase {
    pool: deadpool_diesel::Pool<ConnectionManager, deadpool::managed::Object<ConnectionManager>>,
    metrics: MetricsDatabase,
}

impl SqliteDatabase {
    /// Execute a query within a transaction
    async fn transact<R, Q, M>(&self, msg: M, query: Q) -> Result<R, DatabaseError>
    where
        Q: Send + FnOnce(&mut SqliteConnection) -> Result<R, DatabaseError> + 'static,
        R: Send + 'static,
        M: Send + ToString,
    {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| DatabaseError::Connection(format!("Failed to get connection: {e}")))?;

        conn.interact(|conn| conn.transaction(|conn| query(conn)))
            .await
            .map_err(|err| {
                DatabaseError::QueryExecution(format!("Failed to {}: {}", msg.to_string(), err))
            })?
    }

    /// Execute a query without a transaction
    async fn query<R, Q, M>(&self, msg: M, query: Q) -> Result<R, DatabaseError>
    where
        Q: Send + FnOnce(&mut SqliteConnection) -> Result<R, DatabaseError> + 'static,
        R: Send + 'static,
        M: Send + ToString,
    {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| DatabaseError::Connection(format!("Failed to get connection: {e}")))?;

        conn.interact(move |conn| query(conn)).await.map_err(|err| {
            DatabaseError::QueryExecution(format!("Failed to {}: {}", msg.to_string(), err))
        })?
    }
}

#[async_trait::async_trait]
impl DatabaseBackend for SqliteDatabase {
    async fn connect(
        config: DatabaseConfig,
        metrics: MetricsDatabase,
    ) -> Result<Self, DatabaseError> {
        if !std::path::Path::new(&config.url).exists() && !config.url.contains(":memory:") {
            std::fs::File::create(&config.url).map_err(|e| {
                DatabaseError::Configuration(format!("Failed to create database file: {e}"))
            })?;
        }

        // SQLite `:memory:` DBs are per-connection-isolated — two connections
        // pointing at `:memory:` see two different databases. With a pool of N
        // connections, writes splinter across N isolated DBs and most reads
        // return a partial view, which silently loses note data under load.
        //
        // Two ways to fix for an in-memory DB:
        //   1. `file::memory:?cache=shared` — SQLite URI syntax that makes all
        //      connections share the SAME in-memory DB via shared cache.
        //   2. Pool with `max_size=1` so only one connection exists.
        //
        // We pick #2 for simplicity and portability (URI mode requires the
        // `SQLITE_OPEN_URI` flag to be set on connection open, which is not the
        // driver default). For file-backed URLs, a large pool is appropriate
        // since all connections open the same file.
        let is_in_memory = config.url == ":memory:" || config.url.starts_with("file::memory:");
        let max_size = if is_in_memory { 1 } else { 16 };

        let manager = ConnectionManager::new(&config.url);
        let pool = deadpool_diesel::Pool::builder(manager)
            .max_size(max_size)
            .build()
            .map_err(|e| DatabaseError::Pool(format!("Failed to create connection pool: {e}")))?;

        Ok(Self { pool, metrics })
    }

    #[tracing::instrument(skip(self), fields(operation = "db.store_note"))]
    async fn store_note(&self, note: &StoredNote) -> Result<(), DatabaseError> {
        let timer = self.metrics.db_store_note();

        let new_note = NewNote::from(note);
        self.transact("store note", move |conn| {
            diesel::insert_into(schema::notes::table).values(&new_note).execute(conn)?;
            Ok(())
        })
        .await?;

        timer.finish("ok");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(operation = "db.fetch_notes"))]
    async fn fetch_notes(
        &self,
        tag: NoteTag,
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.fetch_notes_by_tags(&[tag], cursor).await
    }

    #[tracing::instrument(skip(self, tags), fields(operation = "db.fetch_notes_by_tags"))]
    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();

        let cursor_i64: i64 = cursor.try_into().map_err(|_| {
            DatabaseError::QueryExecution("Cursor too large for SQLite".to_string())
        })?;

        if tags.is_empty() {
            timer.finish("ok");
            return Ok(Vec::new());
        }

        let tag_values: Vec<i64> = tags.iter().map(|t| i64::from(t.as_u32())).collect();

        // Single query for all tags runs in ONE DB snapshot, so a concurrent
        // INSERT can't land between per-tag queries and get leapfrogged by the
        // cursor advance. This closes the second half of the pagination race
        // (the monotonic `seq` column closed the timestamp-collision half).
        let notes: Vec<Note> = self
            .transact("fetch notes by tags", move |conn| {
                use schema::notes::dsl::{notes, seq, tag};
                let fetched_notes = notes
                    .filter(tag.eq_any(&tag_values))
                    .filter(seq.gt(cursor_i64))
                    .order(seq.asc())
                    .load::<Note>(conn)?;
                Ok(fetched_notes)
            })
            .await?;

        let mut stored_notes = Vec::new();
        for note in notes {
            let stored_note = StoredNote::try_from(note).map_err(|e| {
                DatabaseError::Deserialization(format!("Failed to deserialize note: {e}"))
            })?;
            stored_notes.push(stored_note);
        }

        timer.finish("ok");

        Ok(stored_notes)
    }

    async fn get_stats(&self) -> Result<(u64, u64), DatabaseError> {
        let (total_notes, total_tags): (i64, i64) = self
            .query("get stats", |conn| {
                #[allow(deprecated)]
                use diesel::dsl::count_distinct;
                use schema::notes::dsl::{notes, tag};

                let total_notes: i64 = notes.count().get_result(conn)?;
                #[allow(deprecated)]
                let total_tags: i64 = notes.select(count_distinct(tag)).first(conn)?;

                Ok((total_notes, total_tags))
            })
            .await?;

        Ok((total_notes.try_into().unwrap_or(0), total_tags.try_into().unwrap_or(0)))
    }

    async fn cleanup_old_notes(&self, retention_days: u32) -> Result<u64, DatabaseError> {
        let cutoff_date = Utc::now() - chrono::Duration::days(i64::from(retention_days));
        let cutoff_timestamp = cutoff_date.timestamp_micros();

        let deleted_count: i64 = self
            .transact("cleanup old notes", move |conn| {
                use schema::notes::dsl::{created_at, notes};
                let count =
                    diesel::delete(notes.filter(created_at.lt(cutoff_timestamp))).execute(conn)?;
                Ok(i64::try_from(count).unwrap_or(0))
            })
            .await?;

        Ok(deleted_count.try_into().unwrap_or(0))
    }

    async fn note_exists(&self, note_id: NoteId) -> Result<bool, DatabaseError> {
        let count: i64 = self
            .query("check note existence", move |conn| {
                use schema::notes::dsl::{id, notes};
                let count =
                    notes.filter(id.eq(&note_id.as_bytes()[..])).count().get_result(conn)?;
                Ok(count)
            })
            .await?;

        Ok(count > 0)
    }
}

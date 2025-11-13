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

        let manager = ConnectionManager::new(&config.url);
        let pool = deadpool_diesel::Pool::builder(manager)
            .max_size(16)
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
        tags: &[NoteTag],
        cursor: u64,
        limit: Option<u32>,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();

        if tags.is_empty() {
            return Ok(Vec::new());
        }

        let cursor_i64: i64 = cursor.try_into().map_err(|_| {
            DatabaseError::QueryExecution("Cursor too large for SQLite".to_string())
        })?;

        let tag_values: Vec<i64> = tags.iter().map(|tag| i64::from(tag.as_u32())).collect();
        let notes: Vec<Note> = self
            .transact("fetch notes", move |conn| {
                use schema::notes::dsl::{created_at, notes, tag};
                let mut query = notes
                    .filter(tag.eq_any(tag_values))
                    .filter(created_at.gt(cursor_i64))
                    .order(created_at.asc())
                    .into_boxed();

                if let Some(limit_val) = limit {
                    let limit_i64 = i64::from(limit_val);
                    query = query.limit(limit_i64);
                }

                let fetched_notes = query.load::<Note>(conn)?;
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

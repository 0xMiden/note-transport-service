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

/// Maximum number of notes returned in a single `fetch_notes` / `fetch_notes_by_tags`
/// response. Bounds memory on both the server (one DB buffer) and the client (one
/// deserialized batch) regardless of how far behind the client's cursor is. A
/// backlogged client paginates naturally by re-calling with the returned cursor.
pub(crate) const FETCH_NOTES_BATCH_SIZE: i64 = 500;

/// Maximum number of expired notes deleted per transaction by `cleanup_old_notes`.
///
/// Retention cleanup is a background job with no deadline, so the batch size trades total
/// throughput for how long the writer lock is held in one go. 10k rows keeps a single batch to a
/// few milliseconds even on a cold page cache, well inside the window in which a concurrent
/// `store_note` will still complete.
const CLEANUP_BATCH_SIZE: i64 = 10_000;

/// Threshold above which a `fetch_notes` cursor is interpreted as a legacy
/// microsecond-timestamp cursor from the pre-`seq` schema and reset to 0.
///
/// Before the `seq`-cursor migration, cursors were `created_at.timestamp_micros()`
/// — values near 1.7×10^15. After migration, cursors are `seq` values starting
/// at 1. Without this reset, any client that stored a cursor before migration
/// would see zero notes forever (until `seq` caught up to their old timestamp,
/// which at realistic insert rates is decades). 10^12 is two orders of magnitude
/// above any plausible `seq` value we'd reach in the lifetime of this deployment,
/// and two orders of magnitude below any microsecond timestamp this decade.
const LEGACY_CURSOR_THRESHOLD: u64 = 1_000_000_000_000;

/// `SQLite` implementation of the database backend
pub struct SqliteDatabase {
    pool: deadpool_diesel::Pool<ConnectionManager, deadpool::managed::Object<ConnectionManager>>,
    metrics: MetricsDatabase,
}

impl SqliteDatabase {
    /// Delete every note created before `cutoff_timestamp`, `batch_size` rows per transaction.
    ///
    /// Deleting the whole expired set in one statement holds the writer lock for its entire
    /// duration and grows the WAL by the size of the transaction. After downtime or a retention
    /// change that set can be millions of rows, which is long enough to push concurrent
    /// `store_note` calls past the busy timeout. Batching bounds both.
    ///
    /// Returns the total number of notes deleted.
    async fn cleanup_notes_before(
        &self,
        cutoff_timestamp: i64,
        batch_size: i64,
    ) -> Result<u64, DatabaseError> {
        let mut total_deleted: u64 = 0;

        loop {
            let deleted: i64 = self
                .transact("cleanup old notes", move |conn| {
                    use schema::notes::dsl::{created_at, notes, seq};

                    // `DELETE ... LIMIT` needs SQLITE_ENABLE_UPDATE_DELETE_LIMIT, which is not
                    // enabled in every libsqlite3 build, so bound the batch by selecting primary
                    // keys first. Oldest first, so a run interrupted part-way still makes forward
                    // progress.
                    let doomed = notes
                        .select(seq)
                        .filter(created_at.lt(cutoff_timestamp))
                        .order(seq.asc())
                        .limit(batch_size)
                        .load::<i64>(conn)?;

                    if doomed.is_empty() {
                        return Ok(0);
                    }

                    let count = diesel::delete(notes.filter(seq.eq_any(doomed))).execute(conn)?;
                    Ok(i64::try_from(count).unwrap_or(0))
                })
                .await?;

            total_deleted = total_deleted.saturating_add(deleted.try_into().unwrap_or(0));

            // A short batch means the expired set is exhausted.
            if deleted < batch_size {
                break;
            }

            // Give writers a chance at the lock between batches.
            tokio::task::yield_now().await;
        }

        Ok(total_deleted)
    }

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
        //   1. `file::memory:?cache=shared` — SQLite URI syntax that makes all connections share
        //      the SAME in-memory DB via shared cache.
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

    #[tracing::instrument(skip(self, note), fields(operation = "db.store_note"))]
    async fn store_note(&self, note: &StoredNote) -> Result<(), DatabaseError> {
        tracing::debug!(note_id = %note.header.id(), tag = note.header.metadata().tag().as_u32(), "db store_note");

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

    async fn fetch_notes(
        &self,
        tag: NoteTag,
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        self.fetch_notes_by_tags(&[tag], cursor).await
    }

    #[tracing::instrument(skip(self, tags), fields(
        operation = "db.fetch_notes_by_tags",
        tag_count = tags.len(),
        cursor = cursor,
        notes_returned = tracing::field::Empty,
    ))]
    async fn fetch_notes_by_tags(
        &self,
        tags: &[NoteTag],
        cursor: u64,
    ) -> Result<Vec<StoredNote>, DatabaseError> {
        let timer = self.metrics.db_fetch_notes();

        // Legacy cursor detection: clients upgraded from the pre-`seq` schema
        // carry microsecond-timestamp cursors; interpret those as 0 so they
        // don't stall forever waiting for `seq` to catch up. Record a metric
        // so operators can see when pre-migration clients are being reset.
        let effective_cursor = if cursor > LEGACY_CURSOR_THRESHOLD {
            self.metrics.db_fetch_notes_legacy_cursor_reset();
            tracing::info!(original_cursor = cursor, "Legacy cursor reset to 0");
            0
        } else {
            cursor
        };

        let cursor_i64: i64 = effective_cursor.try_into().map_err(|_| {
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
        //
        // LIMIT caps response size; a backlogged client paginates by re-calling
        // with the returned cursor until the response is smaller than the limit.
        let notes: Vec<Note> = self
            .transact("fetch notes by tags", move |conn| {
                use schema::notes::dsl::{notes, seq, tag};
                let fetched_notes = notes
                    .filter(tag.eq_any(&tag_values))
                    .filter(seq.gt(cursor_i64))
                    .order(seq.asc())
                    .limit(FETCH_NOTES_BATCH_SIZE)
                    // Name-based column selection (via `Selectable`) so a future
                    // mid-table column insert can't silently misalign fields.
                    .select(Note::as_select())
                    .load(conn)?;
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

        tracing::Span::current().record("notes_returned", stored_notes.len());
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

        self.cleanup_notes_before(cutoff_date.timestamp_micros(), CLEANUP_BATCH_SIZE)
            .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;
    use crate::test_utils::test_note_header;

    /// Store `count` notes, all stamped at `created_at`.
    async fn store_notes_at(db: &SqliteDatabase, count: usize, created_at: chrono::DateTime<Utc>) {
        for _ in 0..count {
            let note = StoredNote {
                header: test_note_header(),
                details: vec![1, 2, 3],
                created_at,
                seq: 0,
                after_block_num: None,
            };
            db.store_note(&note).await.unwrap();
        }
    }

    async fn test_db() -> SqliteDatabase {
        SqliteDatabase::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap()
    }

    /// The expired set is deleted in full even when it spans many batches, and the loop
    /// terminates rather than spinning on the final short batch.
    #[tokio::test]
    async fn cleanup_deletes_every_expired_note_across_batches() {
        let db = test_db().await;
        let old = Utc::now() - chrono::Duration::days(60);

        store_notes_at(&db, 7, old).await;

        // A batch size well below the row count forces several passes, with the last one short.
        let deleted = db.cleanup_notes_before(Utc::now().timestamp_micros(), 2).await.unwrap();

        assert_eq!(deleted, 7);
        assert_eq!(db.get_stats().await.unwrap().0, 0);
    }

    /// Only notes older than the cutoff are deleted; the batching must not run past it.
    #[tokio::test]
    async fn cleanup_keeps_notes_newer_than_the_cutoff() {
        let db = test_db().await;
        let old = Utc::now() - chrono::Duration::days(60);
        let recent = Utc::now();

        store_notes_at(&db, 5, old).await;
        store_notes_at(&db, 3, recent).await;

        let cutoff = (Utc::now() - chrono::Duration::days(30)).timestamp_micros();
        let deleted = db.cleanup_notes_before(cutoff, 2).await.unwrap();

        assert_eq!(deleted, 5);
        assert_eq!(db.get_stats().await.unwrap().0, 3);
    }

    /// Durability must not depend on how the linked libsqlite3 was built, so the pragmas are set
    /// explicitly on every connection. `synchronous=FULL` is 2.
    #[tokio::test]
    async fn connections_are_opened_with_the_configured_durability_pragmas() {
        #[derive(QueryableByName)]
        struct PragmaValue {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            value: i32,
        }

        let db = test_db().await;

        let synchronous = db
            .query("read synchronous pragma", |conn| {
                Ok(diesel::sql_query("SELECT synchronous AS value FROM pragma_synchronous")
                    .load::<PragmaValue>(conn)?
                    .first()
                    .map(|row| row.value)
                    .unwrap_or_default())
            })
            .await
            .unwrap();

        assert_eq!(synchronous, 2, "expected PRAGMA synchronous=FULL");
    }

    /// Nothing to delete must be one empty pass, not an error and not a loop.
    #[tokio::test]
    async fn cleanup_on_an_empty_expired_set_is_a_no_op() {
        let db = test_db().await;
        store_notes_at(&db, 3, Utc::now()).await;

        let cutoff = (Utc::now() - chrono::Duration::days(30)).timestamp_micros();

        assert_eq!(db.cleanup_notes_before(cutoff, 2).await.unwrap(), 0);
        assert_eq!(db.get_stats().await.unwrap().0, 3);
    }
}

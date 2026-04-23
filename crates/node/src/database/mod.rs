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
            commitment_block_num: None,
            note_metadata: None,
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
            commitment_block_num: None,
            note_metadata: None,
        };
        db.store_note(&first).await.unwrap();

        let second = StoredNote {
            header: test_note_header(),
            details: vec![2],
            created_at: Utc::now(),
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        };
        db.store_note(&second).await.unwrap();

        // Fetch everything and assert INSERT order = read order = seq ascending.
        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.len(), 2);
        assert!(
            fetched[0].seq < fetched[1].seq,
            "expected monotonic seq; got {} then {}",
            fetched[0].seq,
            fetched[1].seq
        );
        assert_eq!(fetched[0].details, vec![1]);
        assert_eq!(fetched[1].details, vec![2]);

        // Cursor between the two seqs returns only the second.
        let mid_cursor = u64::try_from(fetched[0].seq).expect("seq is non-negative");
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
            Database::connect(DatabaseConfig::default(), Metrics::default().db)
                .await
                .unwrap(),
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
                    commitment_block_num: None,
                    note_metadata: None,
                })
                .await
                .unwrap();
            });
        }
        while writers.join_next().await.is_some() {}

        let fetched_a = db.fetch_notes(TAG_A.into(), 0).await.unwrap();
        let fetched_b = db.fetch_notes(TAG_B.into(), 0).await.unwrap();
        assert_eq!(
            fetched_a.len() + fetched_b.len(),
            40,
            "all 40 concurrent writes should be visible"
        );

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
            commitment_block_num: None,
            note_metadata: None,
        };

        db.store_note(&note).await.unwrap();

        // cursor=0 is strictly before any assigned seq → should return the note
        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(fetched.len(), 1);
        let stored_seq = fetched[0].seq;
        assert!(stored_seq > 0, "expected seq > 0, got {stored_seq}");

        // cursor = the note's own seq → strictly-greater filter excludes it
        let cursor = u64::try_from(stored_seq).expect("seq is non-negative");
        let after = db.fetch_notes(TAG_LOCAL_ANY.into(), cursor).await.unwrap();
        assert_eq!(after.len(), 0);
    }

    /// Deterministic regression test for the seq-cursor fix.
    ///
    /// Two notes with IDENTICAL `created_at` microseconds (possible on macOS
    /// under concurrent writes; more broadly, any system where wall-clock is
    /// not injective under load). With the old `created_at` cursor, the
    /// client's `rcursor = max(ts)` strict-greater filter rendered the 2nd
    /// note permanently invisible. With the `seq` cursor, each note gets a
    /// distinct monotonic id and both are reachable.
    #[tokio::test]
    async fn test_seq_cursor_survives_identical_created_at() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        let t = Utc::now();
        let note1 = StoredNote {
            header: test_note_header(),
            details: vec![1],
            created_at: t,
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        };
        db.store_note(&note1).await.unwrap();

        // Client's first fetch sees note1, advances cursor using the returned seq.
        let first = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(first.len(), 1);
        let cursor = u64::try_from(first[0].seq).expect("seq is non-negative");

        // A concurrent writer commits note2 with the SAME `created_at`.
        let note2 = StoredNote {
            header: test_note_header(),
            details: vec![2],
            created_at: t,
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        };
        db.store_note(&note2).await.unwrap();

        // With seq cursor: note2 has seq > cursor → returned.
        // With old timestamp cursor: note2.ts == cursor → filtered by strict `>` → LOST.
        let second = db.fetch_notes(TAG_LOCAL_ANY.into(), cursor).await.unwrap();
        assert_eq!(
            second.len(),
            1,
            "seq cursor must return note2 despite identical created_at; got {} notes",
            second.len()
        );
        assert_eq!(second[0].details, vec![2]);
    }

    /// Deterministic regression test for the multi-tag single-snapshot fix.
    ///
    /// Simulates the old gRPC handler's per-tag loop: fetch tag A, then fetch
    /// tag B, then advance client cursor to `max(seq)` across both results.
    /// A concurrent tag-A insert landing between the per-tag queries gets a
    /// seq ABOVE A's max but BELOW B's max — so when the client advances to
    /// B's max seq, the A insert becomes permanently unreachable (its seq is
    /// below the advanced cursor).
    ///
    /// `fetch_notes_by_tags` collapses the loop into a single `tag IN (…)`
    /// query under one snapshot, so no interleave is possible.
    #[tokio::test]
    async fn test_multi_tag_single_snapshot_vs_per_tag_loop() {
        use std::convert::TryFrom;

        const TAG_A: u32 = 0x3d9c_0000;
        const TAG_B: u32 = 0x47ac_0000;

        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        // Seed: one pre-existing tag A note.
        db.store_note(&StoredNote {
            header: test_note_header_with_tag(TAG_A),
            details: vec![1],
            created_at: Utc::now(),
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        })
        .await
        .unwrap();

        // === Simulate the old per-tag loop ===
        // Step 1: fetch tag A.
        let a_result = db.fetch_notes(TAG_A.into(), 0).await.unwrap();
        assert_eq!(a_result.len(), 1);

        // Between the per-tag queries, two concurrent writes commit in order:
        //   (i) a tag A note — this is the one the loop will lose.
        //  (ii) a tag B note — its higher seq will bump the client's cursor
        //       past (i), rendering (i) unreachable on retry.
        db.store_note(&StoredNote {
            header: test_note_header_with_tag(TAG_A),
            details: vec![2],
            created_at: Utc::now(),
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        })
        .await
        .unwrap();
        db.store_note(&StoredNote {
            header: test_note_header_with_tag(TAG_B),
            details: vec![3],
            created_at: Utc::now(),
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        })
        .await
        .unwrap();

        // Step 2: fetch tag B.
        let b_result = db.fetch_notes(TAG_B.into(), 0).await.unwrap();
        assert_eq!(b_result.len(), 1);

        // Client advances cursor to max(seq) across both results, mirroring the
        // gRPC handler's `rcursor` computation.
        let rcursor: u64 = a_result
            .iter()
            .chain(b_result.iter())
            .map(|n| u64::try_from(n.seq).unwrap())
            .max()
            .unwrap_or(0);

        // Client's next per-tag fetches with the advanced cursor.
        let retry_a = db.fetch_notes(TAG_A.into(), rcursor).await.unwrap();
        let retry_b = db.fetch_notes(TAG_B.into(), rcursor).await.unwrap();

        // Per-tag loop sees only 2 of the 3 notes — the interleaved tag A
        // insert with the details=[2] payload is missing.
        let per_tag_visible = a_result.len() + b_result.len() + retry_a.len() + retry_b.len();
        assert_eq!(
            per_tag_visible, 2,
            "per-tag loop must lose the interleaved tag-A insert; visible = {per_tag_visible}"
        );
        assert!(
            !retry_a.iter().any(|n| n.details == vec![2]),
            "the interleaved tag-A insert with details=[2] must be unreachable to the per-tag loop"
        );

        // === The fix: single-snapshot multi-tag query ===
        let snapshot = db.fetch_notes_by_tags(&[TAG_A.into(), TAG_B.into()], 0).await.unwrap();
        assert_eq!(
            snapshot.len(),
            3,
            "fetch_notes_by_tags must return all 3 notes in a single consistent snapshot; got {}",
            snapshot.len()
        );
        assert!(
            snapshot.iter().any(|n| n.details == vec![2]),
            "the interleaved tag-A insert MUST be visible via the single-snapshot query"
        );
    }

    /// Legacy cursor reset: a client carrying a pre-migration microsecond-
    /// timestamp cursor (e.g. 1.7×10^15) would otherwise see 0 notes until
    /// `seq` reached that magnitude, which at realistic rates is decades.
    /// Cursors above `LEGACY_CURSOR_THRESHOLD` are treated as 0.
    #[tokio::test]
    async fn test_fetch_notes_resets_legacy_cursor() {
        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        let note = StoredNote {
            header: test_note_header(),
            details: vec![1, 2, 3, 4],
            created_at: Utc::now(),
            seq: 0,
            commitment_block_num: None,
            note_metadata: None,
        };
        db.store_note(&note).await.unwrap();

        // A realistic "legacy" cursor — microseconds since the epoch, currently
        // ~1.76×10^15. Well above the 10^12 threshold.
        let legacy_cursor: u64 = 1_760_000_000_000_000;
        let fetched = db.fetch_notes(TAG_LOCAL_ANY.into(), legacy_cursor).await.unwrap();
        assert_eq!(
            fetched.len(),
            1,
            "legacy microsecond cursor should be reset to 0, returning the note"
        );

        // Sanity check: a non-legacy cursor above the note's seq should NOT trigger the reset.
        let normal_cursor: u64 = 1_000;
        let empty = db.fetch_notes(TAG_LOCAL_ANY.into(), normal_cursor).await.unwrap();
        assert_eq!(empty.len(), 0, "normal cursor > seq should filter correctly");
    }

    /// Pagination: a response is capped at `FETCH_NOTES_BATCH_SIZE` rows. A
    /// backlogged client sees a bounded batch on each call and advances the
    /// cursor to pick up the rest on the next call.
    #[tokio::test]
    async fn test_fetch_notes_paginates_at_batch_limit() {
        use crate::database::sqlite::FETCH_NOTES_BATCH_SIZE;

        let db = Database::connect(DatabaseConfig::default(), Metrics::default().db)
            .await
            .unwrap();

        // Insert BATCH_SIZE + extra notes for the same tag.
        let extra: usize = 7;
        let total = usize::try_from(FETCH_NOTES_BATCH_SIZE).unwrap() + extra;
        for i in 0..total {
            db.store_note(&StoredNote {
                header: test_note_header(),
                details: vec![(i % 256) as u8],
                created_at: Utc::now(),
                seq: 0,
                commitment_block_num: None,
                note_metadata: None,
            })
            .await
            .unwrap();
        }

        // First fetch from cursor=0 returns exactly BATCH_SIZE rows.
        let first = db.fetch_notes(TAG_LOCAL_ANY.into(), 0).await.unwrap();
        assert_eq!(
            i64::try_from(first.len()).unwrap(),
            FETCH_NOTES_BATCH_SIZE,
            "first batch must be capped at FETCH_NOTES_BATCH_SIZE"
        );

        // Advance cursor to max(seq) and refetch — remaining rows come back.
        let advanced: u64 = first.iter().map(|n| u64::try_from(n.seq).unwrap()).max().unwrap();
        let second = db.fetch_notes(TAG_LOCAL_ANY.into(), advanced).await.unwrap();
        assert_eq!(second.len(), extra, "second batch must contain the remaining {extra} rows");

        // Third fetch drains nothing (nothing left).
        let third_cursor: u64 =
            second.iter().map(|n| u64::try_from(n.seq).unwrap()).max().unwrap_or(advanced);
        let third = db.fetch_notes(TAG_LOCAL_ANY.into(), third_cursor).await.unwrap();
        assert_eq!(third.len(), 0, "drained");

        // Stats reflect every row written.
        let (total_stats, _) = db.get_stats().await.unwrap();
        assert_eq!(usize::try_from(total_stats).unwrap(), total);
    }
}

//! Delivery-invariant test harness for the note-transport store.
//!
//! **Why this exists.** Three separate "notes not delivered" incidents — the
//! `created_at` timestamp-collision race, the `:memory:` pool-isolation bug, and
//! a cursor stranded above a regressed `seq` high-water — were all the SAME
//! invariant violated three different ways:
//!
//! > A note stored under tag `T` is delivered to a fetcher for `T` exactly once
//! > when draining from cursor 0, and is never left permanently unreachable —
//! > regardless of cursor values, concurrency, cleanup, or DB recreation.
//!
//! Example-based unit tests missed all three because they only cover the cases
//! the author imagined. This harness *explores the state space*: it drives
//! random operation sequences (Layer 1), random concurrent schedules (Layer 2),
//! and DB-recreation / cleanup edges (Layer 3) against the REAL backend and
//! asserts the invariant. A fresh bug in this class shows up as a failing seed.
//!
//! Determinism note: the seed replays the OPERATION SEQUENCE exactly. Note
//! identities come from a global RNG and are re-randomized each run, which does
//! not affect the structural cursor/tag/seq invariants under test (a failure
//! reproduces from the op sequence regardless of which id bytes are used).
//!
//! Every randomized test is SEEDED and prints the seed (and, for Layer 1, the
//! greedily-minimized failing op sequence) on failure. Tune with env vars:
//! `NTS_INV_ITERS`, `NTS_INV_LEN`, `NTS_INV_SEED` (Layer 1);
//! `NTS_INV_CONC_ITERS` (Layer 2). Defaults are CI-fast; raise them for a soak.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Duration, Utc};
use miden_protocol::note::NoteHeader;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::{Database, DatabaseConfig};
use crate::metrics::Metrics;
use crate::test_utils::test_note_header_with_tag;
use crate::types::{NoteTag, StoredNote};

/// An RAII SQLite file path under the system temp dir, removed on drop along
/// with its `-wal`/`-shm` sidecars. Avoids a `tempfile` dependency. Cleanup runs
/// on normal drop and on unwinding panics; only a `panic=abort` profile would
/// leak the temp file (acceptable for a test-only helper).
struct TempDbPath {
    path: std::path::PathBuf,
}

impl TempDbPath {
    fn new(tag: u64) -> Self {
        let name = format!("nts-inv-{:x}-{}.sqlite", tag, std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        Self { path }
    }

    fn url(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDbPath {
    fn drop(&mut self) {
        let base = self.path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{base}-wal"));
        let _ = std::fs::remove_file(format!("{base}-shm"));
    }
}

async fn connect_file(path: &TempDbPath) -> Database {
    Database::connect(DatabaseConfig { url: path.url(), retention_days: 30 }, Metrics::default().db)
        .await
        .expect("connect file-backed")
}

/// The server's response batch size (must mirror the real query `LIMIT`).
#[allow(clippy::cast_sign_loss)] // FETCH_NOTES_BATCH_SIZE is a positive constant.
const BATCH: u64 = super::sqlite::FETCH_NOTES_BATCH_SIZE as u64;

/// A small tag alphabet so multi-tag overlaps and collisions actually happen.
/// Local-tag family (`0xc000_00xx`), the same family the existing tests use.
const TAGS: [u32; 3] = [0xc000_0000, 0xc000_0001, 0xc000_0002];

/// A note's stable identity for set/order comparisons (the on-wire note id).
type Id = Vec<u8>;

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn note_id(note: &StoredNote) -> Id {
    note.header.id().as_bytes().to_vec()
}

/// Short hex prefix of an id, for readable diff messages.
fn id_hex(id: &Id) -> String {
    use std::fmt::Write;
    id.iter().take(4).fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Number of distinct ids — used to assert exactly-once (no duplicate delivery).
fn distinct_count(ids: &[Id]) -> usize {
    let mut v = ids.to_vec();
    v.sort();
    v.dedup();
    v.len()
}

/// First index at which two ordered id lists differ, if any.
fn first_diff(a: &[Id], b: &[Id]) -> Option<usize> {
    (0..a.len().max(b.len())).find(|&k| a.get(k) != b.get(k))
}

fn mk_note(header: NoteHeader) -> StoredNote {
    StoredNote {
        header,
        details: vec![0u8],
        created_at: Utc::now(),
        seq: 0,
        after_block_num: None,
    }
}

async fn fresh_mem_db() -> Database {
    Database::connect(DatabaseConfig::default(), Metrics::default().db)
        .await
        .expect("connect :memory:")
}

/// Fetch the way a real client sees it: the DB returns `(notes, effective)`, and
/// the gRPC handler echoes `rcursor = max(effective, max returned seq)`. We
/// replicate that formula so the drain loop and per-fetch assertions use the same
/// client-visible cursor the wallet would store.
///
/// This intentionally mirrors `MidenNoteTransport::fetch_notes` in
/// `node/grpc/mod.rs` rather than calling it (the harness exercises the DB layer
/// directly); the handler's own rcursor logic is pinned by
/// `test_fetch_notes_response_cursor_heals_stranded_client`. `filter_map` on the
/// seq→u64 conversion can't actually drop anything — `seq` is AUTOINCREMENT (≥ 1).
async fn client_fetch(db: &Database, tags: &[NoteTag], cursor: u64) -> (Vec<StoredNote>, u64) {
    let (notes, effective) =
        db.fetch_notes_by_tags(tags, cursor).await.expect("fetch_notes_by_tags");
    let max_seq = notes.iter().filter_map(|n| u64::try_from(n.seq).ok()).max().unwrap_or(0);
    let rcursor = effective.max(max_seq);
    (notes, rcursor)
}

/// Drain the backlog for `tags` by paginating from `start`, returning the
/// delivered ids (in delivery order). Bounded to guard against a
/// non-terminating pagination bug (itself a failure the caller should surface).
async fn drain_from(db: &Database, tags: &[NoteTag], start: u64) -> Vec<Id> {
    let mut cursor = start;
    let mut ids = Vec::new();
    for _ in 0..100_000 {
        let (notes, rcursor) = client_fetch(db, tags, cursor).await;
        if notes.is_empty() {
            break;
        }
        ids.extend(notes.iter().map(note_id));
        if rcursor <= cursor {
            // No forward progress with a non-empty batch: a drain-terminates
            // violation. Emit a diagnostic (so the failure isn't misread as a
            // plain missing-note bug) and stop so we don't loop forever; the
            // completeness / duplicate checks surface the resulting discrepancy.
            eprintln!(
                "drain-terminates violation: non-empty batch but cursor did not advance \
                 (cursor={cursor}, rcursor={rcursor})"
            );
            break;
        }
        cursor = rcursor;
    }
    ids
}

/// Drain the full backlog for `tags` from cursor 0.
async fn drain_all(db: &Database, tags: &[NoteTag]) -> Vec<Id> {
    drain_from(db, tags, 0).await
}

/// Reference model of the store's INTENDED fetch/cursor semantics. Deliberately
/// a plain re-statement of the spec, independent of the SQL implementation.
mod model {
    use super::Id;

    #[derive(Clone)]
    struct Note {
        seq: u64,
        tag: u32,
        id: Id,
    }

    pub struct Model {
        notes: Vec<Note>,
        /// Max `seq` ever assigned this DB lifetime (mirrors AUTOINCREMENT: only
        /// grows within an epoch, resets on recreate — survives deletes).
        high_water: u64,
        batch: u64,
    }

    impl Model {
        pub fn new(batch: u64) -> Self {
            Self { notes: Vec::new(), high_water: 0, batch }
        }

        pub fn store(&mut self, tag: u32, id: Id) {
            self.high_water += 1;
            self.notes.push(Note { seq: self.high_water, tag, id });
        }

        pub fn recreate(&mut self) {
            self.notes.clear();
            self.high_water = 0;
        }

        pub fn high_water(&self) -> u64 {
            self.high_water
        }

        /// The effective cursor after the server's reset rules: legacy µs cursors
        /// (`> 1e12`) and cursors stranded above the high-water reset to 0.
        fn effective(&self, cursor: u64) -> u64 {
            const LEGACY_THRESHOLD: u64 = 1_000_000_000_000;
            // Reset to 0 when the cursor is a legacy µs-timestamp cursor (> 1e12)
            // OR stranded above the current high-water (a regressed seq space
            // after a DB recreation). Both reset reasons collapse to the same
            // effective cursor.
            //
            // Coupling note: on an empty DB `high_water == 0`, so this resets any
            // non-zero cursor — matching the real backend, whose `sqlite_sequence`
            // row for `notes` reports `0` on a freshly-migrated table (see
            // `fresh_db_resets_any_positive_cursor`). The backend's `high_water_seq
            // == None` fail-safe (skip the reset) only fires if the sequence table
            // is genuinely unreadable, which the model does not represent.
            let legacy = cursor > LEGACY_THRESHOLD;
            let stranded = cursor > 0 && cursor > self.high_water;
            if legacy || stranded { 0 } else { cursor }
        }

        /// Predict one `fetch`: the ordered ids returned and the echoed cursor.
        pub fn fetch(&self, tags: &[u32], cursor: u64) -> (Vec<Id>, u64) {
            let effective = self.effective(cursor);
            let mut matched: Vec<&Note> = self
                .notes
                .iter()
                .filter(|n| n.seq > effective && tags.contains(&n.tag))
                .collect();
            matched.sort_by_key(|n| n.seq);
            matched.truncate(self.batch as usize);
            let rcursor = matched.last().map_or(effective, |n| n.seq);
            (matched.iter().map(|n| n.id.clone()).collect(), rcursor)
        }

        /// Every present note for `tags`, in seq order (what a full drain yields).
        pub fn drain_all_ids(&self, tags: &[u32]) -> Vec<Id> {
            let mut matched: Vec<&Note> =
                self.notes.iter().filter(|n| tags.contains(&n.tag)).collect();
            matched.sort_by_key(|n| n.seq);
            matched.iter().map(|n| n.id.clone()).collect()
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Store { tag: u32 },
    Fetch { tags: Vec<u32>, cursor: u64 },
    DrainAll { tags: Vec<u32> },
    Recreate,
}

fn gen_tag_subset(rng: &mut StdRng) -> Vec<u32> {
    loop {
        let subset: Vec<u32> = TAGS.iter().copied().filter(|_| rng.random_bool(0.5)).collect();
        if !subset.is_empty() {
            return subset;
        }
    }
}

/// Pick a cursor that stresses the interesting branches. Any value is a valid
/// test input (the same value goes to both backend and model), so we bias toward
/// 0, valid-range, stranded-above-high-water, and legacy-µs values.
fn gen_cursor(rng: &mut StdRng, high_water: u64) -> u64 {
    match rng.random_range(0..4) {
        0 => 0,
        1 => rng.random_range(0..=high_water + 2),
        2 => high_water + rng.random_range(1..5_000),
        _ => 1_000_000_000_000u64 + rng.random_range(0..1_000_000),
    }
}

fn gen_ops(seed: u64, len: usize) -> Vec<Op> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut ops = Vec::with_capacity(len);
    // Exact within an epoch (no cleanup op in this layer), so it doubles as the
    // high-water estimate for choosing stranded cursors.
    let mut high_water = 0u64;
    for _ in 0..len {
        let op = match rng.random_range(0..100) {
            0..=54 => {
                high_water += 1;
                Op::Store {
                    tag: TAGS[rng.random_range(0..TAGS.len())],
                }
            },
            55..=89 => Op::Fetch {
                tags: gen_tag_subset(&mut rng),
                cursor: gen_cursor(&mut rng, high_water),
            },
            90..=95 => Op::DrainAll { tags: gen_tag_subset(&mut rng) },
            _ => {
                high_water = 0;
                Op::Recreate
            },
        };
        ops.push(op);
    }
    ops
}

fn ntags(tags: &[u32]) -> Vec<NoteTag> {
    tags.iter().map(|t| (*t).into()).collect()
}

/// Run one op sequence against a fresh backend + reference model, asserting they
/// agree per-fetch and that the global no-permanent-invisibility invariant holds
/// at the end. Returns `Err(description)` on the first violation.
async fn run_sequence(ops: &[Op]) -> Result<(), String> {
    let mut db = fresh_mem_db().await;
    let mut model = model::Model::new(BATCH);

    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Store { tag } => {
                let header = test_note_header_with_tag(*tag);
                let id = header.id().as_bytes().to_vec();
                db.store_note(&mk_note(header))
                    .await
                    .map_err(|e| format!("op {i} Store: {e:?}"))?;
                model.store(*tag, id);
            },
            Op::Fetch { tags, cursor } => {
                let (real_notes, real_cursor) = client_fetch(&db, &ntags(tags), *cursor).await;
                let real_ids: Vec<Id> = real_notes.iter().map(note_id).collect();
                let (model_ids, model_cursor) = model.fetch(tags, *cursor);
                if real_ids != model_ids {
                    let diff = first_diff(&real_ids, &model_ids).unwrap_or(0);
                    return Err(format!(
                        "op {i} Fetch(tags={tags:?}, cursor={cursor}): backend returned {} ids, \
                         model {} ids; first diff at index {diff}: backend={:?}, model={:?}",
                        real_ids.len(),
                        model_ids.len(),
                        real_ids.get(diff).map(id_hex),
                        model_ids.get(diff).map(id_hex),
                    ));
                }
                if real_cursor != model_cursor {
                    return Err(format!(
                        "op {i} Fetch(tags={tags:?}, cursor={cursor}): backend rcursor={real_cursor}, \
                         model rcursor={model_cursor}"
                    ));
                }
            },
            Op::DrainAll { tags } => {
                let mut real = drain_all(&db, &ntags(tags)).await;
                if distinct_count(&real) != real.len() {
                    return Err(format!(
                        "op {i} DrainAll(tags={tags:?}): duplicate delivery — {} ids drained, {} distinct",
                        real.len(),
                        distinct_count(&real)
                    ));
                }
                let mut expected = model.drain_all_ids(tags);
                real.sort();
                expected.sort();
                if real != expected {
                    return Err(format!(
                        "op {i} DrainAll(tags={tags:?}): backend drained {} ids, model has {} present",
                        real.len(),
                        expected.len()
                    ));
                }
            },
            Op::Recreate => {
                db = fresh_mem_db().await;
                model.recreate();
            },
        }
    }

    // Global invariant: draining every tag from cursor 0 must yield exactly the
    // present set — nothing lost, nothing duplicated, nothing permanently invisible.
    let mut real = drain_all(&db, &ntags(&TAGS)).await;
    if distinct_count(&real) != real.len() {
        return Err(format!(
            "final completeness: duplicate delivery — {} ids drained, {} distinct",
            real.len(),
            distinct_count(&real)
        ));
    }
    let mut expected = model.drain_all_ids(&TAGS);
    real.sort();
    expected.sort();
    if real != expected {
        return Err(format!(
            "final completeness: {} notes reachable via drain-from-0, model has {} present",
            real.len(),
            expected.len()
        ));
    }

    // Independent stranded-heal oracle (does NOT consult `model.effective`): a
    // cursor stranded above the high-water must recover exactly the same set as
    // draining from 0 — i.e. the reset fully heals it. This catches a
    // stranded-heal regression even if the reference model mirrored the same bug,
    // and it exercises whatever epoch state the random recreates left behind.
    let stranded_start = model.high_water() + 1_000_000;
    let mut stranded = drain_from(&db, &ntags(&TAGS), stranded_start).await;
    stranded.sort();
    if stranded != real {
        return Err(format!(
            "stranded-heal: drain from stranded cursor {stranded_start} yielded {} ids, \
             drain from 0 yielded {} — the stranded cursor did not fully heal",
            stranded.len(),
            real.len()
        ));
    }
    Ok(())
}

/// Greedily minimize a failing op sequence: repeatedly drop any single op that
/// still leaves the sequence failing, to fixpoint. Runs only on failure.
async fn shrink(ops: &[Op]) -> Vec<Op> {
    let mut current = ops.to_vec();
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = 0;
        while i < current.len() {
            let mut candidate = current.clone();
            candidate.remove(i);
            if run_sequence(&candidate).await.is_err() {
                current = candidate; // still fails without op i — keep the shorter one
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    current
}

/// Layer 1 — the reference model and the real backend must agree over random
/// operation sequences (store / fetch-with-cursor / drain / recreate). This is
/// the exploration test: it finds UNKNOWN semantic bugs, not just the ones we
/// already fixed.
#[tokio::test]
async fn model_matches_backend_over_random_sequences() {
    let iters = env_u64("NTS_INV_ITERS", 128);
    let len = env_u64("NTS_INV_LEN", 40) as usize;
    let base_seed = env_u64("NTS_INV_SEED", 0xc0ff_ee12_3456_789a);

    for i in 0..iters {
        let seed = base_seed ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let ops = gen_ops(seed, len);
        if let Err(msg) = run_sequence(&ops).await {
            let minimal = shrink(&ops).await;
            panic!(
                "delivery-invariant violation (seed={seed:#018x}, iter={i})\n  {msg}\n  \
                 minimal failing sequence ({} ops):\n{minimal:#?}",
                minimal.len()
            );
        }
    }
}

/// Arbiter for a disputed review finding: what does the backend actually do with
/// a positive cursor on a freshly-migrated, never-written table? Empirically its
/// `sqlite_sequence` row for `notes` reports high-water 0 (not absent), so ANY
/// positive cursor is stranded-above-high-water and resets to 0 — an empty DB
/// heals a leftover cursor rather than echoing it back. The reference model
/// relies on exactly this (it predicts a reset when `high_water == 0`).
#[tokio::test]
async fn fresh_db_resets_any_positive_cursor() {
    let db = fresh_mem_db().await;
    let (notes, effective) = db
        .fetch_notes_by_tags(&[TAGS[0].into()], 5_000)
        .await
        .expect("fetch on fresh db");
    assert!(notes.is_empty(), "a fresh DB has no notes");
    assert_eq!(effective, 0, "a positive cursor on an empty DB resets to 0 (high-water is 0)");
}

/// Store `NOTES_PER_WRITER` notes per writer concurrently, with concurrent
/// fetchers interleaving reads, then assert a final drain-from-0 delivers every
/// stored note exactly once. Panics with the seed on violation.
async fn concurrent_completeness_once(seed: u64, file_backed: bool) {
    // Default burst 8×80 = 640 > BATCH(500), so the drains paginate across pages
    // under live inserts (where cross-page cursor-advance races would show).
    let writers_n = env_u64("NTS_INV_CONC_WRITERS", 8) as usize;
    let notes_per_writer = env_u64("NTS_INV_CONC_NOTES", 80) as usize;
    let fetchers_n = 3;

    let tmp = if file_backed { Some(TempDbPath::new(seed)) } else { None };
    let db = Arc::new(match &tmp {
        Some(path) => connect_file(path).await,
        None => fresh_mem_db().await,
    });

    // No test-level warm-up: `SqliteDatabase::connect` now eagerly runs the
    // schema migration on one connection before returning, so a burst of ~16
    // fresh connections no longer stampedes migration DDL. This test exercises
    // that guarantee rather than papering over the cold-start race.

    // Concurrent fetchers are an ORACLE, not decoration: each drain pass must be
    // internally duplicate-free (one pass advances a strictly-increasing cursor,
    // so it can't return a note twice even under concurrent inserts), and every
    // id a fetcher observes must be a real stored id (checked against `expected`
    // after the join). Their panics are propagated.
    let stop = Arc::new(AtomicBool::new(false));
    let mut fetchers = Vec::new();
    for _ in 0..fetchers_n {
        let db = db.clone();
        let stop = stop.clone();
        fetchers.push(tokio::spawn(async move {
            let all = ntags(&TAGS);
            let mut observed: BTreeSet<Id> = BTreeSet::new();
            while !stop.load(Ordering::Relaxed) {
                let pass = drain_all(&db, &all).await;
                assert_eq!(
                    distinct_count(&pass),
                    pass.len(),
                    "duplicate id within a single concurrent drain pass"
                );
                observed.extend(pass);
                tokio::task::yield_now().await;
            }
            observed
        }));
    }

    let mut writers = Vec::new();
    for w in 0..writers_n {
        let db = db.clone();
        writers.push(tokio::spawn(async move {
            let mut rng =
                StdRng::seed_from_u64(seed ^ (w as u64).wrapping_mul(0x0123_4567_89ab_cdef));
            let mut ids = Vec::with_capacity(notes_per_writer);
            for _ in 0..notes_per_writer {
                let tag = TAGS[rng.random_range(0..TAGS.len())];
                let header = test_note_header_with_tag(tag);
                ids.push(header.id().as_bytes().to_vec());
                db.store_note(&mk_note(header)).await.expect("concurrent store");
            }
            ids
        }));
    }

    let mut expected: Vec<Id> = Vec::new();
    for handle in writers {
        expected.extend(handle.await.expect("writer join"));
    }
    stop.store(true, Ordering::Relaxed);

    // Propagate fetcher panics (an intra-pass duplicate, a read/deserialize
    // panic) and collect every id any fetcher observed.
    let mut reader_ids: BTreeSet<Id> = BTreeSet::new();
    for handle in fetchers {
        reader_ids.extend(handle.await.expect("fetcher join"));
    }
    let expected_set: BTreeSet<&Id> = expected.iter().collect();
    for id in &reader_ids {
        assert!(
            expected_set.contains(id),
            "concurrent read surfaced an id that was never stored \
             (seed={seed:#018x}, file_backed={file_backed})"
        );
    }

    // Final quiescent drain: every stored note delivered exactly once.
    let mut delivered = drain_all(&db, &ntags(&TAGS)).await;
    assert_eq!(
        distinct_count(&delivered),
        delivered.len(),
        "duplicate delivery under concurrency (seed={seed:#018x}, file_backed={file_backed})"
    );
    expected.sort();
    delivered.sort();
    assert_eq!(
        delivered,
        expected,
        "completeness violation under concurrency: {} delivered vs {} stored \
         (seed={seed:#018x}, file_backed={file_backed})",
        delivered.len(),
        expected.len()
    );
}

/// Layer 2 (file-backed) — the real production path: a 16-connection pool over a
/// WAL file. Concurrent writers + readers must never lose or duplicate a note.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_all_delivered_file_backed() {
    let iters = env_u64("NTS_INV_CONC_ITERS", 3);
    for i in 0..iters {
        concurrent_completeness_once(0xf11e_0000 ^ i.wrapping_mul(0x9e37_79b9), true).await;
    }
}

/// Layer 2 (`:memory:`) — guards the pool-size=1 clamp that fixed the `:memory:`
/// per-connection isolation bug: with a naive pool>1 the concurrent writes would
/// splinter across isolated in-memory DBs and the drain would lose most of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_all_delivered_in_memory() {
    let iters = env_u64("NTS_INV_CONC_ITERS", 3);
    for i in 0..iters {
        concurrent_completeness_once(0x0ee0_0000 ^ i.wrapping_mul(0x9e37_79b9), false).await;
    }
}

/// Layer 3 — a cursor stranded across a DB recreation recovers the new epoch.
/// Models the real incident: epoch 1 advances the seq high-water, the backing
/// volume is recreated (fresh AUTOINCREMENT), and a client still holding the old
/// (now above-max) cursor must still receive the new epoch's notes and heal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stranded_cursor_recovers_after_db_recreation() {
    let tag: NoteTag = TAGS[0].into();

    // Epoch 1: advance the high-water past the eventual epoch-2 max.
    let epoch1 = TempDbPath::new(0xe1);
    let db1 = connect_file(&epoch1).await;
    for _ in 0..50 {
        db1.store_note(&mk_note(test_note_header_with_tag(TAGS[0]))).await.unwrap();
    }
    let (_notes, epoch1_cursor) = client_fetch(&db1, &[tag], 0).await;
    assert_eq!(epoch1_cursor, 50, "epoch-1 high-water");
    drop(db1);

    // Epoch 2: a fresh DB (seq restarts at 1) with fewer notes than epoch1_cursor.
    let epoch2 = TempDbPath::new(0xe2);
    let db2 = connect_file(&epoch2).await;
    let mut new_ids = Vec::new();
    for _ in 0..10 {
        let header = test_note_header_with_tag(TAGS[0]);
        new_ids.push(header.id().as_bytes().to_vec());
        db2.store_note(&mk_note(header)).await.unwrap();
    }

    // The stranded epoch-1 cursor (50 > epoch-2 max of 10) must reset and recover
    // all 10 new-epoch notes.
    let (recovered, healed_cursor) = client_fetch(&db2, &[tag], epoch1_cursor).await;
    let mut got: Vec<Id> = recovered.iter().map(note_id).collect();
    got.sort();
    new_ids.sort();
    assert_eq!(got, new_ids, "stranded client must recover the full new epoch");
    assert_eq!(healed_cursor, 10, "healed cursor is the new-epoch max seq");

    // The next poll from the healed cursor returns nothing new (converged).
    let (none, next) = client_fetch(&db2, &[tag], healed_cursor).await;
    assert!(none.is_empty(), "no phantom notes after heal");
    assert_eq!(next, healed_cursor, "steady state converged");
}

/// Layer 3 — `cleanup_old_notes` removes only retention-expired notes and never
/// lowers the AUTOINCREMENT high-water, so fresh notes stay reachable and a
/// caught-up cursor is never mistaken for stranded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_removes_only_expired_and_preserves_high_water() {
    let tmp = TempDbPath::new(0xc1ea);
    let db = connect_file(&tmp).await;
    let tag: NoteTag = TAGS[0].into();

    // 5 notes created 40 days ago, then 5 fresh ones (seq 1..=10).
    let old_at = Utc::now() - Duration::days(40);
    for _ in 0..5 {
        let mut note = mk_note(test_note_header_with_tag(TAGS[0]));
        note.created_at = old_at;
        db.store_note(&note).await.unwrap();
    }
    let mut fresh_ids = Vec::new();
    for _ in 0..5 {
        let header = test_note_header_with_tag(TAGS[0]);
        fresh_ids.push(header.id().as_bytes().to_vec());
        db.store_note(&mk_note(header)).await.unwrap();
    }

    // Retain 30 days → the 5 expired notes are deleted, the 5 fresh remain.
    let deleted = db.cleanup_old_notes(30).await.unwrap();
    assert_eq!(deleted, 5, "exactly the expired notes are removed");

    let mut reachable = drain_all(&db, &[tag]).await;
    reachable.sort();
    fresh_ids.sort();
    assert_eq!(reachable, fresh_ids, "fresh notes remain reachable after cleanup");

    // A cursor at the preserved high-water (10) must NOT be treated as stranded.
    let (none, cur) = client_fetch(&db, &[tag], 10).await;
    assert!(none.is_empty());
    assert_eq!(cur, 10, "cursor at the preserved high-water must not reset");
}

/// A backlog larger than one response batch must paginate correctly: every note
/// delivered exactly once across pages, terminating.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_paginates_across_batches_without_loss_or_dup() {
    let tmp = TempDbPath::new(0xba7c);
    let db = connect_file(&tmp).await;
    let tag: NoteTag = TAGS[0].into();

    let total = (BATCH + BATCH / 2) as usize; // 1.5 batches → forces >1 page
    let mut stored = Vec::with_capacity(total);
    for _ in 0..total {
        let header = test_note_header_with_tag(TAGS[0]);
        stored.push(header.id().as_bytes().to_vec());
        db.store_note(&mk_note(header)).await.unwrap();
    }

    // The first page is capped at the batch size.
    let (first, _) = client_fetch(&db, &[tag], 0).await;
    assert_eq!(first.len() as u64, BATCH, "first page is capped at the batch size");

    let delivered = drain_all(&db, &[tag]).await;
    let mut deduped = delivered.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), delivered.len(), "no duplicates across pages");

    let mut delivered_sorted = delivered;
    delivered_sorted.sort();
    stored.sort();
    assert_eq!(delivered_sorted, stored, "every note delivered exactly once across pages");
}

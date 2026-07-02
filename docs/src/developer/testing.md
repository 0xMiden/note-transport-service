# Testing

Alongside the per-feature unit tests, the node ships a **delivery-invariant test
harness** (`crates/node/src/database/invariant_tests.rs`) that exercises the
note store's core guarantee against randomized inputs.

## The invariant

> A note stored under tag `T` is delivered to a fetcher for `T` **exactly once**
> when draining from cursor `0`, and is **never left permanently unreachable** —
> regardless of cursor values, concurrency, cleanup, or DB recreation.

Three separate "notes not delivered" incidents were all this one invariant
violated in different ways:

- the `created_at` microsecond-timestamp **collision race** (fixed by the
  monotonic `seq` cursor);
- the `:memory:` **pool-isolation** bug (fixed by clamping the `:memory:` pool to
  a single connection);
- a cursor **stranded** above a regressed `seq` high-water after a DB recreation
  (fixed by resetting such cursors to `0` and healing the echoed cursor).

Example-based unit tests missed all three because they only cover the cases the
author imagined. The harness instead *explores the state space*.

## The layers

- **Layer 1 — model / property.** A reference model of the intended
  fetch/cursor semantics is run in lockstep with the real `Database` over random
  operation sequences (store / fetch-with-cursor / drain / recreate). Any
  divergence — a stranding, off-by-one, pagination, or reset bug — fails the run.
- **Layer 2 — seeded concurrency.** Many concurrent writers and readers hammer a
  real file-backed 16-connection pool (and a `:memory:` pool) and a final drain
  must return every stored note exactly once. This is where the pool-isolation
  and timestamp races lived.
- **Layer 3 — lifecycle edges.** Cross-recreation stranded-cursor recovery,
  `cleanup_old_notes` retention, and cross-batch pagination.

## Running

The randomized tests are **CI-fast by default** and **seeded** — a failure
prints the seed (and, for Layer 1, the greedily-minimized failing operation
sequence) so it replays deterministically.

```bash
# Default (fast) run — part of `cargo test`.
cargo test -p miden-note-transport-node invariant_tests

# Deeper soak: more Layer-1 iterations / longer sequences / more concurrency rounds.
NTS_INV_ITERS=5000 NTS_INV_LEN=80 NTS_INV_CONC_ITERS=25 \
  cargo test -p miden-note-transport-node invariant_tests -- --nocapture

# Reproduce a specific reported failure.
NTS_INV_SEED=0x... cargo test -p miden-note-transport-node model_matches_backend_over_random_sequences
```

When adding behavior to the store, prefer extending the reference model and the
operation set over adding a one-off example test — that keeps the exploration
covering the new behavior too.

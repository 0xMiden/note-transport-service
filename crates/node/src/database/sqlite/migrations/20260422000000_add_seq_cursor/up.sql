-- Replace the `notes` table with a monotonic INTEGER PRIMARY KEY (`seq`).
--
-- The previous `fetch_notes` pagination used `created_at` (microsecond
-- timestamp) as the cursor, which is vulnerable to a race when multiple
-- `send_note` inserts happen concurrently with a multi-tag fetch: a note can
-- be inserted with a `created_at` lower than the `rcursor` returned for an
-- earlier tag, leaving the note strictly under the next fetch's cursor and
-- thus permanently unreachable.
--
-- `seq` is assigned at INSERT-commit time, monotonic, and survives VACUUM
-- because of AUTOINCREMENT — INSERT order defines read order regardless of
-- wall clock. See NotesNotDelivered.md in miden-wallet for the full writeup.

CREATE TABLE notes_new (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id BLOB NOT NULL UNIQUE,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

INSERT INTO notes_new (id, tag, header, details, created_at)
SELECT id, tag, header, details, created_at FROM notes
ORDER BY created_at ASC;

DROP TABLE notes;
ALTER TABLE notes_new RENAME TO notes;

-- Compound (tag, seq) supports the hot pagination query
--     WHERE tag = ? AND seq > ? ORDER BY seq ASC
-- so the whole operation is index-only as the table grows.
CREATE INDEX idx_notes_tag_seq ON notes(tag, seq);
-- Kept for cleanup_old_notes (DELETE WHERE created_at < ?).
CREATE INDEX idx_notes_created_at ON notes(created_at);

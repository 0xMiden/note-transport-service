ALTER TABLE notes RENAME TO notes_legacy;

CREATE TABLE notes (
    seq INTEGER PRIMARY KEY,
    envelope_digest BLOB,
    id BLOB NOT NULL,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    after_block_num INTEGER
) STRICT;

INSERT INTO notes (seq, id, tag, header, details, created_at, after_block_num)
SELECT seq, id, tag, header, details, created_at, after_block_num
FROM notes_legacy
ORDER BY seq;

DROP TABLE notes_legacy;

CREATE INDEX idx_notes_tag_seq ON notes(tag, seq);
CREATE INDEX idx_notes_created_at ON notes(created_at);

CREATE TABLE storage_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    next_cursor INTEGER NOT NULL,
    retained_bytes INTEGER NOT NULL CHECK (retained_bytes >= 0)
) STRICT;

INSERT INTO storage_metadata (singleton, next_cursor, retained_bytes)
SELECT
    1,
    COALESCE(MAX(seq), 0) + 1,
    COALESCE(SUM(LENGTH(header) + LENGTH(details)), 0)
FROM notes;

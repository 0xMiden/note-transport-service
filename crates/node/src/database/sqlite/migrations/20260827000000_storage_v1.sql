CREATE TABLE notes (
    seq INTEGER PRIMARY KEY,
    id BLOB NOT NULL UNIQUE,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    after_block_num INTEGER
) STRICT;

CREATE INDEX idx_notes_tag_seq ON notes(tag, seq);
CREATE INDEX idx_notes_created_at ON notes(created_at);

CREATE TABLE storage_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    next_cursor INTEGER NOT NULL,
    retained_bytes INTEGER NOT NULL CHECK (retained_bytes >= 0)
) STRICT;

INSERT INTO storage_metadata (singleton, next_cursor, retained_bytes)
VALUES (1, 1, 0);

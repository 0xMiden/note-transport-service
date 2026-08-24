CREATE TABLE IF NOT EXISTS notes (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id BLOB NOT NULL UNIQUE,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    after_block_num INTEGER
) STRICT;

CREATE INDEX IF NOT EXISTS idx_notes_tag_seq ON notes(tag, seq);
CREATE INDEX IF NOT EXISTS idx_notes_created_at ON notes(created_at);

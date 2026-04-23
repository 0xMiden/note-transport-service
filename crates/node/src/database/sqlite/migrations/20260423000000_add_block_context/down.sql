CREATE TABLE notes_backup (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id BLOB NOT NULL UNIQUE,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
INSERT INTO notes_backup SELECT seq, id, tag, header, details, created_at FROM notes;
DROP TABLE notes;
ALTER TABLE notes_backup RENAME TO notes;
CREATE INDEX idx_notes_tag_seq ON notes(tag, seq);
CREATE INDEX idx_notes_created_at ON notes(created_at);

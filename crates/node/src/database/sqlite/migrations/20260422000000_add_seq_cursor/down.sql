CREATE TABLE notes_old (
    id BLOB PRIMARY KEY,
    tag INTEGER NOT NULL,
    header BLOB NOT NULL,
    details BLOB NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

INSERT INTO notes_old (id, tag, header, details, created_at)
SELECT id, tag, header, details, created_at FROM notes;

DROP TABLE notes;
ALTER TABLE notes_old RENAME TO notes;

CREATE INDEX idx_notes_tag ON notes(tag);
CREATE INDEX idx_notes_created_at ON notes(created_at);

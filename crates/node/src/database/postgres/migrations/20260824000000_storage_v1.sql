CREATE TABLE notes (
    seq BIGINT PRIMARY KEY,
    id BYTEA NOT NULL UNIQUE,
    tag BIGINT NOT NULL,
    header BYTEA NOT NULL,
    details BYTEA NOT NULL,
    created_at BIGINT NOT NULL,
    after_block_num BIGINT
);

CREATE INDEX idx_notes_tag_seq ON notes(tag, seq);
CREATE INDEX idx_notes_created_at ON notes(created_at);

CREATE TABLE storage_metadata (
    singleton BOOLEAN PRIMARY KEY CHECK (singleton),
    next_cursor BIGINT NOT NULL,
    retained_bytes BIGINT NOT NULL CHECK (retained_bytes >= 0)
);

INSERT INTO storage_metadata (singleton, next_cursor, retained_bytes)
VALUES (TRUE, 1, 0);

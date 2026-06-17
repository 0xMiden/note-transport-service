use chrono::DateTime;
use diesel::prelude::*;
use miden_protocol::utils::serde::{Deserializable, Serializable};

use super::schema::notes;
use crate::database::DatabaseError;
use crate::types::{NoteHeader, StoredNote};

#[derive(Queryable, Selectable, Debug, Clone)]
#[diesel(table_name = notes)]
#[allow(clippy::struct_field_names)]
pub struct Note {
    pub seq: i64,
    pub id: Vec<u8>,
    pub tag: i64,
    pub header: Vec<u8>,
    pub details: Vec<u8>,
    pub created_at: i64,
    pub after_block_num: Option<i64>,
}

// `seq` is omitted from `NewNote`: SQLite auto-assigns it on INSERT via
// INTEGER PRIMARY KEY AUTOINCREMENT, and we want INSERT-commit order (not
// anything caller-provided) to define read order.
#[derive(Insertable)]
#[diesel(table_name = notes)]
pub struct NewNote {
    pub id: Vec<u8>,
    pub tag: i64,
    pub header: Vec<u8>,
    pub details: Vec<u8>,
    pub created_at: i64,
    pub after_block_num: Option<i64>,
}

impl From<&StoredNote> for NewNote {
    fn from(note: &StoredNote) -> Self {
        Self {
            id: note.header.id().as_bytes().to_vec(),
            tag: i64::from(note.header.metadata().tag().as_u32()),
            header: note.header.to_bytes(),
            details: note.details.clone(),
            created_at: note.created_at.timestamp_micros(),
            after_block_num: note.after_block_num.map(i64::from),
        }
    }
}

impl TryFrom<Note> for StoredNote {
    type Error = DatabaseError;

    fn try_from(note: Note) -> std::result::Result<Self, Self::Error> {
        let created_at = DateTime::from_timestamp_micros(note.created_at).ok_or_else(|| {
            DatabaseError::Deserialization(format!(
                "Invalid timestamp microseconds: {}",
                note.created_at
            ))
        })?;

        let header = NoteHeader::read_from_bytes(&note.header).map_err(|e| {
            DatabaseError::Deserialization(format!("Failed to deserialize header: {e}"))
        })?;

        Ok(StoredNote {
            header,
            details: note.details,
            created_at,
            seq: note.seq,
            after_block_num: note
                .after_block_num
                .map(|n| {
                    u32::try_from(n).map_err(|_| {
                        DatabaseError::Deserialization(format!("Invalid after_block_num: {n}"))
                    })
                })
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use miden_protocol::utils::serde::Serializable;

    use super::*;
    use crate::database::DatabaseError;
    use crate::test_utils::test_note_header;

    /// `TryFrom<Note> for StoredNote` runs on every fetched row. It must map
    /// each column to the right field and preserve `after_block_num` in both
    /// the present and absent cases.
    #[test]
    fn test_note_converts_to_stored_note() {
        let header = test_note_header();
        let created_at = Utc::now().timestamp_micros();

        // after_block_num present: every field maps through.
        let row = Note {
            seq: 7,
            id: header.id().as_bytes().to_vec(),
            tag: 0,
            header: header.to_bytes(),
            details: vec![1, 2, 3],
            created_at,
            after_block_num: Some(100),
        };
        let stored = StoredNote::try_from(row).expect("valid row must convert");
        assert_eq!(stored.seq, 7);
        assert_eq!(stored.header.id().as_bytes(), header.id().as_bytes());
        assert_eq!(stored.details, vec![1, 2, 3]);
        assert_eq!(stored.created_at.timestamp_micros(), created_at);
        assert_eq!(stored.after_block_num, Some(100));

        // after_block_num absent: the optional stays None.
        let bare = Note {
            seq: 8,
            id: header.id().as_bytes().to_vec(),
            tag: 0,
            header: header.to_bytes(),
            details: vec![],
            created_at,
            after_block_num: None,
        };
        let stored_bare = StoredNote::try_from(bare).expect("valid row must convert");
        assert_eq!(stored_bare.after_block_num, None);
    }

    /// An `after_block_num` outside the `u32` domain (only reachable via a
    /// corrupt or tampered DB row) is rejected with a `DatabaseError` rather
    /// than panicking or silently truncating.
    #[test]
    fn test_out_of_range_after_block_num_is_rejected() {
        let header = test_note_header();
        let row = Note {
            seq: 1,
            id: header.id().as_bytes().to_vec(),
            tag: 0,
            header: header.to_bytes(),
            details: vec![],
            created_at: Utc::now().timestamp_micros(),
            after_block_num: Some(i64::from(u32::MAX) + 1),
        };

        match StoredNote::try_from(row) {
            Err(DatabaseError::Deserialization(msg)) => {
                assert!(msg.contains("Invalid after_block_num"), "unexpected message: {msg}");
            },
            other => panic!("expected Deserialization error, got: {other:?}"),
        }
    }
}

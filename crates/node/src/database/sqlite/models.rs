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
    pub commitment_block_num: Option<i64>,
    pub note_metadata: Option<Vec<u8>>,
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
    pub commitment_block_num: Option<i64>,
    pub note_metadata: Option<Vec<u8>>,
}

impl From<&StoredNote> for NewNote {
    fn from(note: &StoredNote) -> Self {
        Self {
            id: note.header.id().as_bytes().to_vec(),
            tag: i64::from(note.header.metadata().tag().as_u32()),
            header: note.header.to_bytes(),
            details: note.details.clone(),
            created_at: note.created_at.timestamp_micros(),
            commitment_block_num: note.commitment_block_num.map(i64::from),
            note_metadata: note.note_metadata.clone(),
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
            commitment_block_num: note
                .commitment_block_num
                .map(|n| {
                    u32::try_from(n).map_err(|_| {
                        DatabaseError::Deserialization(format!("Invalid commitment_block_num: {n}"))
                    })
                })
                .transpose()?,
            note_metadata: note.note_metadata,
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

    /// The `TryFrom<Note> for StoredNote` conversion rejects a
    /// `commitment_block_num` that exceeds `u32::MAX`. This guards against
    /// corrupt or tampered DB rows where the `i64` column holds a value
    /// outside the `u32` domain. Without this test the conversion guard at
    /// line 74-78 is dead code from a coverage perspective.
    #[test]
    fn test_block_context_rejects_out_of_range_value() {
        let header = test_note_header();
        let raw_note = Note {
            seq: 1,
            id: header.id().as_bytes().to_vec(),
            tag: 0,
            header: header.to_bytes(),
            details: vec![],
            created_at: Utc::now().timestamp_micros(),
            commitment_block_num: Some(i64::from(u32::MAX) + 1),
            note_metadata: None,
        };

        let result = StoredNote::try_from(raw_note);
        assert!(result.is_err(), "commitment_block_num above u32::MAX must be rejected");
        match result.unwrap_err() {
            DatabaseError::Deserialization(msg) => {
                assert!(
                    msg.contains("Invalid commitment_block_num"),
                    "unexpected error message: {msg}"
                );
            },
            other => panic!("expected DatabaseError::Deserialization, got: {other:?}"),
        }
    }
}

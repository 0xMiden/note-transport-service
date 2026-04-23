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

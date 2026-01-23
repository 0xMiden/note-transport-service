use chrono::DateTime;
use diesel::prelude::*;
use miden_protocol::utils::serde::{Deserializable, Serializable};

use super::schema::notes;
use crate::database::DatabaseError;
use crate::types::{NoteHeader, StoredNote};

#[derive(Queryable, Selectable, Debug, Clone)]
#[diesel(table_name = notes)]
pub struct Note {
    pub id: Vec<u8>,
    pub tag: i64,
    pub header: Vec<u8>,
    pub details: Vec<u8>,
    pub created_at: i64,
}

#[derive(Insertable)]
#[diesel(table_name = notes)]
pub struct NewNote {
    pub id: Vec<u8>,
    pub tag: i64,
    pub header: Vec<u8>,
    pub details: Vec<u8>,
    pub created_at: i64,
}

impl From<&StoredNote> for NewNote {
    fn from(note: &StoredNote) -> Self {
        Self {
            id: note.header.id().as_bytes().to_vec(),
            tag: i64::from(note.header.metadata().tag().as_u32()),
            header: note.header.to_bytes(),
            details: note.details.clone(),
            created_at: note.created_at.timestamp_micros(),
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
        })
    }
}

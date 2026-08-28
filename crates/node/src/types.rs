use chrono::{DateTime, Utc};
use miden_note_transport_proto::miden_note_transport::v1::TransportNote;
pub use miden_protocol::Felt;
pub use miden_protocol::account::AccountId;
pub use miden_protocol::block::BlockNumber;
pub use miden_protocol::note::{
    Note,
    NoteDetails,
    NoteHeader,
    NoteId,
    NoteInclusionProof,
    NoteTag,
    NoteType,
};
use miden_protocol::utils::serde::Serializable;

/// A note stored in the database
#[derive(Debug, Clone)]
pub struct StoredNote {
    /// Note header
    pub header: NoteHeader,
    /// Serialized plaintext note details.
    pub details: Vec<u8>,
    /// Reference timestamp
    pub created_at: DateTime<Utc>,
    /// Monotonic sequence number assigned by the database at INSERT commit.
    ///
    /// This is the canonical cursor value used by `fetch_notes` pagination.
    /// Untouched when constructing a `StoredNote` for insertion — the DB
    /// assigns the real value via `INTEGER PRIMARY KEY AUTOINCREMENT`.
    pub seq: i64,
    /// Block number where the note commitment was included on-chain.
    pub after_block_num: Option<u32>,
}

impl From<StoredNote> for TransportNote {
    fn from(snote: StoredNote) -> Self {
        Self {
            header: snote.header.to_bytes(),
            details: snote.details,
            after_block_num: snote.after_block_num,
        }
    }
}

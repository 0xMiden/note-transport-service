use miden_protocol::account::AccountId;
use miden_protocol::note::{NoteHeader, NoteId, NoteMetadata, NoteTag, NoteType};
use miden_protocol::testing::account_id::ACCOUNT_ID_MAX_ZEROES;
use miden_protocol::{Felt, Word};
use rand::Rng;

/// Generate a random [`NoteId`]
pub fn random_note_id() -> NoteId {
    let mut rng = rand::rng();

    let recipient = Word::from([
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
    ]);
    let asset_commitment = Word::from([
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
        Felt::new(rng.random::<u64>()),
    ]);

    NoteId::new(recipient, asset_commitment)
}

/// Tag value for local notes
pub const TAG_LOCAL_ANY: u32 = 0xc000_0000;

/// Generate a private [`NoteHeader`] with random sender
pub fn test_note_header() -> NoteHeader {
    test_note_header_with_tag(TAG_LOCAL_ANY)
}

/// Generate a private [`NoteHeader`] with random sender and a specified tag
pub fn test_note_header_with_tag(tag_value: u32) -> NoteHeader {
    let id = random_note_id();
    let sender = AccountId::try_from(ACCOUNT_ID_MAX_ZEROES).unwrap();
    let note_type = NoteType::Private;
    let tag = NoteTag::new(tag_value);

    let metadata = NoteMetadata::new(sender, note_type).with_tag(tag);

    NoteHeader::new(id, metadata)
}

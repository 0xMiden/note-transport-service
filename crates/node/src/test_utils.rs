use miden_protocol::account::AccountId;
use miden_protocol::note::{
    Note,
    NoteAssets,
    NoteHeader,
    NoteRecipient,
    NoteScript,
    NoteStorage,
    NoteTag,
    NoteType,
    PartialNoteMetadata,
};
use miden_protocol::testing::account_id::ACCOUNT_ID_MAX_ZEROES;
use miden_protocol::{Felt, Word};
use rand::RngExt;

/// Generate a private note with a local tag.
pub fn test_note() -> Note {
    test_note_with_tag(TAG_LOCAL_ANY)
}

/// Generate a private note with a specified tag.
pub fn test_note_with_tag(tag_value: u32) -> Note {
    let mut rng = rand::rng();
    let serial_num = Word::from([
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
    ]);
    let recipient =
        NoteRecipient::new(serial_num, NoteScript::mock(), NoteStorage::new(Vec::new()).unwrap());
    let sender = AccountId::try_from(ACCOUNT_ID_MAX_ZEROES).unwrap();
    let metadata =
        PartialNoteMetadata::new(sender, NoteType::Private).with_tag(NoteTag::new(tag_value));

    Note::new(NoteAssets::default(), metadata, recipient)
}

/// Tag value for local notes
pub const TAG_LOCAL_ANY: u32 = 0xc000_0000;

/// Generate a private [`NoteHeader`] with random sender
pub fn test_note_header() -> NoteHeader {
    test_note_header_with_tag(TAG_LOCAL_ANY)
}

/// Generate a private [`NoteHeader`] with random sender and a specified tag
pub fn test_note_header_with_tag(tag_value: u32) -> NoteHeader {
    *test_note_with_tag(tag_value).header()
}

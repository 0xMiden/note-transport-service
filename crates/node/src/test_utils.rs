use miden_protocol::account::AccountId;
use miden_protocol::note::{
    NoteAttachments,
    NoteDetailsCommitment,
    NoteHeader,
    NoteMetadata,
    NoteTag,
    NoteType,
    PartialNoteMetadata,
};
use miden_protocol::testing::account_id::ACCOUNT_ID_MAX_ZEROES;
use miden_protocol::{Felt, Word};
use rand::Rng;

/// Generate a random [`NoteDetailsCommitment`]
pub fn random_note_details_commitment() -> NoteDetailsCommitment {
    let mut rng = rand::rng();

    let recipient = Word::from([
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
    ]);
    let asset_commitment = Word::from([
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
        Felt::from(rng.random::<u32>()),
    ]);

    NoteDetailsCommitment::from_raw_commitments(recipient, asset_commitment)
}

/// Tag value for local notes
pub const TAG_LOCAL_ANY: u32 = 0xc000_0000;

/// Generate a private [`NoteHeader`] with random sender
pub fn test_note_header() -> NoteHeader {
    test_note_header_with_tag(TAG_LOCAL_ANY)
}

/// Generate a private [`NoteHeader`] with random sender and a specified tag
pub fn test_note_header_with_tag(tag_value: u32) -> NoteHeader {
    let details_commitment = random_note_details_commitment();
    let sender = AccountId::try_from(ACCOUNT_ID_MAX_ZEROES).unwrap();
    let note_type = NoteType::Private;
    let tag = NoteTag::new(tag_value);

    let partial_metadata = PartialNoteMetadata::new(sender, note_type).with_tag(tag);
    let metadata = NoteMetadata::new(partial_metadata, &NoteAttachments::empty());

    NoteHeader::new(details_commitment, metadata)
}

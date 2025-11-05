//! Mock client

use std::time::Duration;

use anyhow::Result;
use miden_lib::account::wallets::BasicWallet;
use miden_note_transport_proto::miden_note_transport::miden_note_transport_client::MidenNoteTransportClient;
use miden_note_transport_proto::miden_note_transport::{
    FetchNotesRequest,
    SendNoteRequest,
    TransportNote,
};
use miden_objects::account::{Account, AccountBuilder, AccountStorageMode};
use miden_objects::address::{Address, AddressInterface, RoutingParameters};
use miden_objects::crypto::dsa::eddsa_25519::SecretKey;
use miden_objects::crypto::ies::{SealedMessage, SealingKey, UnsealingKey};
use miden_objects::note::{Note, NoteDetails, NoteHeader, NoteTag};
use miden_objects::utils::{Deserializable, Serializable};
use miden_testing::Auth;
use rand::Rng;
use tonic::Request;
use tonic::transport::Channel;

pub struct Client {
    pub grpc: MidenNoteTransportClient<Channel>,
    pub unsealing_key: UnsealingKey,
    pub address: Address,
}

impl Client {
    pub async fn init(endpoint: String, timeout_ms: u64) -> Result<Self> {
        let channel = tonic::transport::Endpoint::try_from(endpoint)?
            .timeout(Duration::from_millis(timeout_ms))
            .connect()
            .await?;
        let grpc = MidenNoteTransportClient::new(channel);

        let account = mock_account();

        let secret_key = SecretKey::with_rng(&mut rand::rng());

        let public_key = secret_key.public_key();
        let encryption_key = SealingKey::X25519XChaCha20Poly1305(public_key);
        let address = Address::new(account.id())
            .with_routing_parameters(
                RoutingParameters::new(AddressInterface::BasicWallet)
                    .with_encryption_key(encryption_key),
            )
            .unwrap();

        let unsealing_key = UnsealingKey::X25519XChaCha20Poly1305(secret_key);

        Ok(Self { grpc, unsealing_key, address })
    }

    pub async fn send_note(&mut self, note: Note, address: &Address) -> Result<()> {
        let header = *note.header();
        let details: NoteDetails = note.into();

        let details_enc = address
            .encryption_key()
            .unwrap()
            .seal_bytes(&mut rand::rng(), &details.to_bytes())
            .unwrap();

        let request = SendNoteRequest {
            note: Some(TransportNote {
                header: header.to_bytes(),
                details: details_enc.to_bytes(),
            }),
        };

        self.grpc.send_note(Request::new(request)).await?;

        Ok(())
    }

    pub async fn fetch_notes(&mut self, tags: &[NoteTag], cursor: u64) -> Result<(Vec<Note>, u64)> {
        let tags_int = tags.iter().map(NoteTag::as_u32).collect();
        let request = FetchNotesRequest { tags: tags_int, cursor };

        let response = self.grpc.fetch_notes(Request::new(request)).await?;

        let response = response.into_inner();

        let mut notes = Vec::new();

        for pnote in response.notes {
            let sealed_msg = SealedMessage::read_from_bytes(&pnote.details)?;
            // try decrypt, if fail just ignore
            let Ok(details_bytes) = self.unsealing_key.unseal_bytes(sealed_msg) else {
                continue;
            };
            let details = NoteDetails::read_from_bytes(&details_bytes)?;
            let header = NoteHeader::read_from_bytes(&pnote.header)?;

            let note = Note::new(
                details.assets().clone(),
                *header.metadata(),
                details.recipient().clone(),
            );
            notes.push(note);
        }

        Ok((notes, response.cursor))
    }
}

pub fn mock_account() -> Account {
    let mut rng = rand::rng();
    AccountBuilder::new(rng.random())
        .storage_mode(AccountStorageMode::Private)
        .with_component(BasicWallet)
        .with_auth_component(Auth::BasicAuth)
        .build()
        .unwrap()
}

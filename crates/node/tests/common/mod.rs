mod client;

use miden_lib::note::create_p2id_note;
use miden_note_transport_node::node::grpc::GrpcServerConfig;
use miden_note_transport_node::{Node, NodeConfig};
use miden_objects::Felt;
use miden_objects::account::AccountId;
use miden_objects::address::{Address, AddressId};
use miden_objects::crypto::rand::RpoRandomCoin;
use miden_objects::note::{Note, NoteType};
use rand::RngCore;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use self::client::Client;

pub async fn spawn_test_server(port: u16) -> JoinHandle<()> {
    let config = NodeConfig {
        grpc: GrpcServerConfig { port, ..Default::default() },
        ..Default::default()
    };

    let server = Node::init(config).await.unwrap();
    let handle = tokio::spawn(server.entrypoint());
    // Wait for startup
    sleep(Duration::from_millis(100)).await;
    handle
}

pub async fn mock_client(port: u16) -> Client {
    let timeout_ms = 1000;
    let url = format!("http://127.0.0.1:{port}");
    Client::init(url, timeout_ms).await.unwrap()
}

pub fn mock_note_p2id_with_addresses(sender: &Address, target: &Address) -> Note {
    let mut randrng = rand::rng();
    let seed: [Felt; 4] = core::array::from_fn(|_| Felt::new(randrng.next_u64()));
    let mut rng = RpoRandomCoin::new(seed.into());
    let sender_id = addrid_to_accid(&sender.id());
    let target_id = addrid_to_accid(&target.id());
    create_p2id_note(sender_id, target_id, vec![], NoteType::Private, Felt::default(), &mut rng)
        .unwrap()
}

pub fn addrid_to_accid(addrid: &AddressId) -> AccountId {
    if let AddressId::AccountId(accid) = addrid {
        *accid
    } else {
        panic!()
    }
}

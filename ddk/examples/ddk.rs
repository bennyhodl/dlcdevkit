use async_trait::async_trait;
use ddk::{builder::DdkBuilder, DlcDevKitDlcManager};
use ddk::{DdkTransport, DdkOracle, DdkStorage};
use tokio::sync::Mutex;
use std::sync::Arc;

#[derive(Clone)]
pub struct MockTransport;

#[async_trait]
impl DdkTransport for MockTransport {
    fn name(&self) -> String {
        "mock-transport".into()
    }
    async fn listen(&self) {
        println!("Listening with MockTransport")
    }
    async fn receive_dlc_message(&self, _manager: &Arc<Mutex<DlcDevKitDlcManager>>) {
        println!("Handling DLC messages with MockTransport")
    }
}

#[derive(Clone)]
struct MockStorage;
impl DdkStorage for MockStorage {}

#[derive(Clone)]
struct MockOracle;
impl DdkOracle for MockOracle {
    fn name(&self) -> String {
        "mock-oracle".into()
    }
}

impl dlc_manager::Oracle for MockOracle {
    fn get_public_key(&self) -> bitcoin::key::XOnlyPublicKey {
        todo!("Trait inherited from rust-dlc")
    }

    fn get_attestation(&self, _event_id: &str) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        todo!("Trait inherited from rust-dlc")
    }

    fn get_announcement(&self, _event_id: &str) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        todo!("Trait inherited from rust-dlc") 
    }
}

type ApplicationDdk = ddk::DlcDevKit<MockTransport, MockStorage, MockOracle>;

#[tokio::main]
async fn main() {
    let transport = Arc::new(MockTransport {});
    let storage = Arc::new(MockStorage {});
    let oracle_client = Arc::new(MockOracle {});
    let ddk: ApplicationDdk = DdkBuilder::new()
        .set_name("dlcdevkit")
        .set_esplora_url(ddk::ESPLORA_HOST)
        .set_network(bitcoin::Network::Regtest)
        .set_transport(transport.clone())
        .set_storage(storage.clone())
        .set_oracle(oracle_client.clone())
        .finish()
        .await
        .unwrap();


    let wallet = ddk.wallet.new_external_address();

    assert!(wallet.is_ok());

    ddk.start().await.expect("nope");

    loop {}
}

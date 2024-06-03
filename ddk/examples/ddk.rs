use async_trait::async_trait;
use ddk::{builder::DdkBuilder, DlcDevKitDlcManager};
use ddk::{DdkOracle, DdkStorage, DdkTransport};
use std::sync::Arc;
use tokio::sync::Mutex;

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

impl dlc_manager::Storage for MockStorage {
    fn get_channel(
        &self,
        _channel_id: &dlc_manager::ChannelId,
    ) -> Result<Option<dlc_manager::channel::Channel>, dlc_manager::error::Error> {
        todo!()
    }

    fn get_contract(
        &self,
        _id: &dlc_manager::ContractId,
    ) -> Result<Option<dlc_manager::contract::Contract>, dlc_manager::error::Error> {
        todo!()
    }

    fn get_contracts(
        &self,
    ) -> Result<Vec<dlc_manager::contract::Contract>, dlc_manager::error::Error> {
        todo!()
    }

    fn get_chain_monitor(
        &self,
    ) -> Result<Option<dlc_manager::chain_monitor::ChainMonitor>, dlc_manager::error::Error> {
        todo!()
    }

    fn upsert_channel(
        &self,
        _channel: dlc_manager::channel::Channel,
        _contract: Option<dlc_manager::contract::Contract>,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn delete_channel(
        &self,
        _channel_id: &dlc_manager::ChannelId,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn create_contract(
        &self,
        _contract: &dlc_manager::contract::offered_contract::OfferedContract,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn delete_contract(
        &self,
        _id: &dlc_manager::ContractId,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn update_contract(
        &self,
        _contract: &dlc_manager::contract::Contract,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn get_contract_offers(
        &self,
    ) -> Result<
        Vec<dlc_manager::contract::offered_contract::OfferedContract>,
        dlc_manager::error::Error,
    > {
        todo!()
    }

    fn get_signed_channels(
        &self,
        _channel_state: Option<dlc_manager::channel::signed_channel::SignedChannelStateType>,
    ) -> Result<Vec<dlc_manager::channel::signed_channel::SignedChannel>, dlc_manager::error::Error>
    {
        todo!()
    }

    fn get_signed_contracts(
        &self,
    ) -> Result<
        Vec<dlc_manager::contract::signed_contract::SignedContract>,
        dlc_manager::error::Error,
    > {
        todo!()
    }

    fn get_offered_channels(
        &self,
    ) -> Result<Vec<dlc_manager::channel::offered_channel::OfferedChannel>, dlc_manager::error::Error>
    {
        todo!()
    }

    fn persist_chain_monitor(
        &self,
        _monitor: &dlc_manager::chain_monitor::ChainMonitor,
    ) -> Result<(), dlc_manager::error::Error> {
        todo!()
    }

    fn get_confirmed_contracts(
        &self,
    ) -> Result<
        Vec<dlc_manager::contract::signed_contract::SignedContract>,
        dlc_manager::error::Error,
    > {
        todo!()
    }

    fn get_preclosed_contracts(
        &self,
    ) -> Result<Vec<dlc_manager::contract::PreClosedContract>, dlc_manager::error::Error> {
        todo!()
    }
}

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

    fn get_attestation(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAttestation, dlc_manager::error::Error> {
        todo!("Trait inherited from rust-dlc")
    }

    fn get_announcement(
        &self,
        _event_id: &str,
    ) -> Result<dlc_messages::oracle_msgs::OracleAnnouncement, dlc_manager::error::Error> {
        todo!("Trait inherited from rust-dlc")
    }
}

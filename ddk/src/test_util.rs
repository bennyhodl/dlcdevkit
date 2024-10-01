use bitcoin::{address::NetworkChecked, bip32::Xpriv, key::rand::Fill, Address, Amount, Network};
use chrono::{DateTime, Days, Local, TimeDelta};
use ddk_payouts::enumeration::create_contract_input;
use dlc::EnumerationPayout;
use dlc_manager::{
    contract::contract_input::ContractInput, manager::Manager, ContractId, Storage,
    SystemTimeProvider,
};
use kormir::OracleAnnouncement;
use std::{path::PathBuf, str::FromStr, sync::Arc, thread::sleep, time::Duration};

use crate::{
    builder::DdkBuilder,
    chain::EsploraClient,
    config::{DdkConfig, SeedConfig},
    oracle::KormirOracleClient,
    storage::SledStorage,
    transport::lightning::LightningTransport,
    wallet::DlcDevKitWallet,
    DdkOracle, DlcDevKit,
};
use bitcoincore_rpc::RpcApi;

type TestDlcDevKit = DlcDevKit<LightningTransport, SledStorage, KormirOracleClient>;

#[rstest::fixture]
pub async fn test_ddk() -> (TestSuite, TestSuite, OracleAnnouncement, ContractInput) {
    let test = TestSuite::new("send_offer", 1778).await;
    let test_two = TestSuite::new("sender_offer_two", 1779).await;
    // test.ddk.start().unwrap();
    // test_two.ddk.start().unwrap();

    // let test_two_transport = test_two.ddk.transport.clone();
    // let test_transport = test.ddk.transport.clone();
    //
    // let pubkey_one = test.ddk.transport.node_id;
    // test_two_transport
    //     .connect_outbound(pubkey_one, "127.0.0.1:1778")
    //     .await;

    // tokio::time::sleep(Duration::from_millis(200)).await;

    // let peers = test_transport.ln_peer_manager().list_peers();
    // assert!(peers.len() > 0);

    let announcement = create_oracle_announcement().await;
    let contract_input = contract_input(&announcement);

    let node_one_address = test.ddk.wallet.new_external_address().unwrap().address;
    let node_two_address = test_two.ddk.wallet.new_external_address().unwrap().address;

    fund_addresses(&node_one_address, &node_two_address);

    test.ddk.wallet.sync().unwrap();
    test_two.ddk.wallet.sync().unwrap();

    (test, test_two, announcement, contract_input)
}

fn fund_addresses(
    node_one_address: &Address<NetworkChecked>,
    node_two_address: &Address<NetworkChecked>,
) {
    let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let client = bitcoincore_rpc::Client::new("http://127.0.0.1:18443", auth).unwrap();
    client
        .send_to_address(
            node_one_address,
            Amount::from_btc(1.0).unwrap(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    client
        .send_to_address(
            node_two_address,
            Amount::from_btc(1.0).unwrap(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    generate_blocks(5)
}

pub fn generate_blocks(num: u64) {
    let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let client = bitcoincore_rpc::Client::new("http://127.0.0.1:18443", auth).unwrap();
    let previous_height = client.get_block_count().unwrap();

    let address = client.get_new_address(None, None).unwrap().assume_checked();
    client.generate_to_address(num, &address).unwrap();
    let mut cur_block_height = previous_height;
    while cur_block_height < previous_height + num {
        sleep(Duration::from_secs(5));
        cur_block_height = client.get_block_count().unwrap();
    }
}

async fn create_oracle_announcement() -> OracleAnnouncement {
    let kormir = KormirOracleClient::new("http://127.0.0.1:8082")
        .await
        .unwrap();

    let expiry = TimeDelta::seconds(30);
    let timestamp: u32 = Local::now()
        .checked_add_signed(expiry)
        .unwrap()
        .timestamp()
        .try_into()
        .unwrap();

    let event_id = kormir
        .create_event(vec!["rust".to_string(), "go".to_string()], timestamp)
        .await
        .unwrap();

    kormir.get_announcement_async(&event_id).await.unwrap()
}

pub fn contract_input(announcement: &OracleAnnouncement) -> ContractInput {
    create_contract_input(
        vec![
            EnumerationPayout {
                outcome: "rust".to_string(),
                payout: dlc::Payout {
                    offer: 100_000,
                    accept: 0,
                },
            },
            EnumerationPayout {
                outcome: "go".to_string(),
                payout: dlc::Payout {
                    offer: 0,
                    accept: 100_000,
                },
            },
        ],
        50_000,
        50_000,
        1,
        announcement.oracle_public_key.clone().to_string(),
        announcement.oracle_event.event_id.clone(),
    )
}

type DlcManager = Arc<
    Manager<
        Arc<DlcDevKitWallet<SledStorage>>,
        Arc<
            dlc_manager::CachedContractSignerProvider<
                Arc<DlcDevKitWallet<SledStorage>>,
                dlc_manager::SimpleSigner,
            >,
        >,
        Arc<EsploraClient>,
        Arc<SledStorage>,
        Arc<KormirOracleClient>,
        Arc<SystemTimeProvider>,
        Arc<DlcDevKitWallet<SledStorage>>,
        dlc_manager::SimpleSigner,
    >,
>;

pub struct TestSuite {
    pub ddk: TestDlcDevKit,
    pub path: String,
}

impl TestSuite {
    pub async fn new(name: &str, port: u16) -> TestSuite {
        let storage_path = format!("tests/data/{name}");
        std::fs::create_dir_all(storage_path.clone()).expect("couldn't create file");
        let config = DdkConfig {
            storage_path: PathBuf::from_str(&storage_path).unwrap(),
            network: Network::Regtest,
            esplora_host: "http://127.0.0.1:30000".to_string(),
            seed_config: SeedConfig::File(storage_path.clone()),
        };
        let transport =
            Arc::new(LightningTransport::new(&config.seed_config, port, config.network).unwrap());
        let storage = Arc::new(
            SledStorage::new(config.storage_path.join("sled_db").to_str().unwrap()).unwrap(),
        );
        let oracle = Arc::new(
            KormirOracleClient::new("http://127.0.0.1:8082")
                .await
                .unwrap(),
        );

        let ddk = Self::create_ddk(
            name,
            config,
            transport.clone(),
            storage.clone(),
            oracle.clone(),
        )
        .await;

        TestSuite {
            ddk,
            path: storage_path,
        }
    }

    async fn create_ddk(
        name: &str,
        config: DdkConfig,
        transport: Arc<LightningTransport>,
        storage: Arc<SledStorage>,
        oracle: Arc<KormirOracleClient>,
    ) -> TestDlcDevKit {
        let ddk: TestDlcDevKit = DdkBuilder::new()
            .set_name(name)
            .set_config(config)
            .set_oracle(oracle)
            .set_transport(transport)
            .set_storage(storage)
            .finish()
            .unwrap();

        ddk
    }

    pub fn create_wallet(name: &str) -> DlcDevKitWallet<SledStorage> {
        let path = format!("tests/data/{name}");
        let storage = Arc::new(SledStorage::new(&path).unwrap());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xpriv = Xpriv::new_master(Network::Regtest, &entropy).unwrap();
        DlcDevKitWallet::new(
            "test".into(),
            xpriv,
            "http://localhost:30000",
            Network::Regtest,
            &path,
            storage.clone(),
        )
        .unwrap()
    }
}

impl Drop for TestSuite {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).expect("Couldn't remove wallet dir");
    }
}

pub fn wait_for_offer_is_stored(contract_id: ContractId, storage: Arc<SledStorage>) {
    let mut tries = 0;
    let mut time = Duration::from_secs(1);
    loop {
        if tries == 5 {
            panic!("Never found contract.");
        }
        let offers = storage.get_contract_offers().unwrap();
        let offer = offers.iter().find(|o| o.id == contract_id);
        if offer.is_some() {
            break;
        }
        sleep(time);
        time = time + Duration::from_secs(5);
        tries = tries + 1;
    }
}

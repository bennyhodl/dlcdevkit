use bitcoin::{bip32::Xpriv, key::rand::Fill, Network};
use chrono::{Days, Local};
use dlc_manager::{manager::Manager, SystemTimeProvider};
use kormir::OracleAnnouncement;
use std::{path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use crate::{
    builder::DdkBuilder,
    chain::EsploraClient,
    config::{DdkConfig, SeedConfig},
    oracle::KormirOracleClient,
    storage::SledStorageProvider,
    transport::lightning::LightningTransport,
    wallet::DlcDevKitWallet,
    DdkTransport, DlcDevKit,
};

type TestDlcDevKit = DlcDevKit<LightningTransport, SledStorageProvider, KormirOracleClient>;

#[rstest::fixture]
pub async fn test_ddk() -> (TestSuite, TestSuite, OracleAnnouncement) {
    let test = TestSuite::new("send_offer", 1778).await;
    let test_two = TestSuite::new("sender_offer_two", 1779).await;
    test.ddk.start().unwrap();
    test_two.ddk.start().unwrap();

    let test_two_transport = test_two.ddk.transport.clone();
    let test_transport = test.ddk.transport.clone();

    let pubkey_one = test.ddk.transport.node_id;
    test_two_transport
        .connect_outbound(pubkey_one, "127.0.0.1:1778")
        .await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let peers = test_transport.ln_peer_manager().list_peers();
    assert!(peers.len() > 0);

    let timestamp: u32 = Local::now()
        .checked_add_days(Days::new(1))
        .unwrap()
        .timestamp()
        .try_into()
        .unwrap();
    let ann = test
        .ddk
        .oracle
        .create_event(vec!["rust".to_string(), "go".to_string()], timestamp)
        .await
        .unwrap();
    (test, test_two, ann)
}

type DlcManager = Arc<
    Manager<
        Arc<DlcDevKitWallet<SledStorageProvider>>,
        Arc<
            dlc_manager::CachedContractSignerProvider<
                Arc<DlcDevKitWallet<SledStorageProvider>>,
                dlc_manager::SimpleSigner,
            >,
        >,
        Arc<EsploraClient>,
        Arc<SledStorageProvider>,
        Arc<KormirOracleClient>,
        Arc<SystemTimeProvider>,
        Arc<DlcDevKitWallet<SledStorageProvider>>,
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
        print!("transport");
        let storage = Arc::new(
            SledStorageProvider::new(config.storage_path.join("sled_db").to_str().unwrap())
                .unwrap(),
        );
        print!("storage");
        let oracle = Arc::new(
            KormirOracleClient::new("http://127.0.0.1:8082")
                .await
                .unwrap(),
        );
        print!("oracle");

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
        storage: Arc<SledStorageProvider>,
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

    pub fn create_wallet(name: &str) -> DlcDevKitWallet<SledStorageProvider> {
        let path = format!("tests/data/{name}");
        let storage = Arc::new(SledStorageProvider::new(&path).unwrap());
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

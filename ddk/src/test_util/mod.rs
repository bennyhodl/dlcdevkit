use bitcoin::{
    address::NetworkChecked,
    bip32::Xpriv,
    key::{
        rand::{Fill, Rng},
        Secp256k1,
    },
    secp256k1::All,
    Address, Amount, Network,
};
use chrono::{Local, TimeDelta};
use ddk_dlc::EnumerationPayout;
use ddk_manager::{
    contract::contract_input::ContractInput, manager::Manager, ContractId, Storage,
    SystemTimeProvider,
};
use ddk_messages::oracle_msgs::OracleAnnouncement;
use ddk_payouts::enumeration::create_contract_input;
use kormir::{storage::MemoryStorage as KormirMemoryStorage, Oracle};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use crate::{
    builder::Builder, chain::EsploraClient, oracle::memory::MemoryOracle,
    storage::memory::MemoryStorage, transport::memory::MemoryTransport, wallet::DlcDevKitWallet,
    DlcDevKit,
};
use bitcoincore_rpc::RpcApi;

type TestDlcDevKit = DlcDevKit<MemoryTransport, MemoryStorage, MemoryOracle>;
pub struct TestWallet(pub DlcDevKitWallet, String);

#[rstest::fixture]
pub async fn test_ddk() -> (
    TestSuite,
    TestSuite,
    (u32, OracleAnnouncement),
    ContractInput,
) {
    let secp = Secp256k1::new();
    let oracle = Arc::new(MemoryOracle::default());

    let test = TestSuite::new(&secp, "send_offer", oracle.clone()).await;
    let test_two = TestSuite::new(&secp, "sender_offer_two", oracle.clone()).await;

    let announcement = create_oracle_announcement(oracle.clone()).await;
    let contract_input = contract_input(&announcement.1);

    let node_one_address = test.ddk.wallet.new_external_address().unwrap().address;
    let node_two_address = test_two.ddk.wallet.new_external_address().unwrap().address;

    fund_addresses(&node_one_address, &node_two_address);

    test.ddk.wallet.sync().unwrap();
    test_two.ddk.wallet.sync().unwrap();

    (test, test_two, announcement, contract_input)
}

pub fn fund_addresses(
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
    tracing::warn!("Generating {} blocks.", num);
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

pub async fn create_oracle_announcement(oracle: Arc<MemoryOracle>) -> (u32, OracleAnnouncement) {
    let expiry = TimeDelta::seconds(15);
    let timestamp: u32 = Local::now()
        .checked_add_signed(expiry)
        .unwrap()
        .timestamp()
        .try_into()
        .unwrap();

    oracle
        .oracle
        .create_enum_event(
            "test".into(),
            vec!["rust".to_string(), "go".to_string()],
            timestamp,
        )
        .await
        .unwrap()
}

pub fn contract_input(announcement: &OracleAnnouncement) -> ContractInput {
    create_contract_input(
        vec![
            EnumerationPayout {
                outcome: "rust".to_string(),
                payout: ddk_dlc::Payout {
                    offer: 100_000,
                    accept: 0,
                },
            },
            EnumerationPayout {
                outcome: "go".to_string(),
                payout: ddk_dlc::Payout {
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
        Arc<DlcDevKitWallet>,
        Arc<
            ddk_manager::CachedContractSignerProvider<
                Arc<DlcDevKitWallet>,
                ddk_manager::SimpleSigner,
            >,
        >,
        Arc<EsploraClient>,
        Arc<MemoryStorage>,
        Arc<MemoryOracle>,
        Arc<SystemTimeProvider>,
        Arc<DlcDevKitWallet>,
        ddk_manager::SimpleSigner,
    >,
>;

pub struct TestSuite {
    pub ddk: TestDlcDevKit,
    pub path: String,
}

impl TestSuite {
    pub async fn new(secp: &Secp256k1<All>, name: &str, oracle: Arc<MemoryOracle>) -> TestSuite {
        let storage_path = format!("tests/data/{name}");
        std::fs::create_dir_all(storage_path.clone()).expect("couldn't create file");
        let seed =
            xprv_from_path(PathBuf::from_str(&storage_path).unwrap(), Network::Regtest).unwrap();
        let esplora_host = "http://127.0.0.1:30000".to_string();

        let transport = Arc::new(MemoryTransport::new(secp));
        let storage = Arc::new(MemoryStorage::new());

        let ddk = Self::create_ddk(
            name,
            transport.clone(),
            storage.clone(),
            oracle.clone(),
            seed.private_key.secret_bytes(),
            esplora_host,
            Network::Regtest,
        )
        .await;

        TestSuite {
            ddk,
            path: storage_path,
        }
    }

    async fn create_ddk(
        name: &str,
        transport: Arc<MemoryTransport>,
        storage: Arc<MemoryStorage>,
        oracle: Arc<MemoryOracle>,
        seed_bytes: [u8; 32],
        esplora_host: String,
        network: Network,
    ) -> TestDlcDevKit {
        let ddk: TestDlcDevKit = Builder::new()
            .set_network(network)
            .set_seed_bytes(seed_bytes)
            .set_esplora_host(esplora_host)
            .set_name(name)
            .set_oracle(oracle)
            .set_transport(transport)
            .set_storage(storage)
            .finish()
            .await
            .unwrap();

        ddk
    }

    pub fn create_wallet(name: &str) -> TestWallet {
        let path = format!("tests/data/{name}");
        let storage = Arc::new(MemoryStorage::new());
        let mut entropy = [0u8; 64];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xpriv = Xpriv::new_master(Network::Regtest, &entropy).unwrap();
        TestWallet(
            DlcDevKitWallet::new(
                "test".into(),
                &xpriv.private_key.secret_bytes(),
                "http://localhost:30000",
                Network::Regtest,
                storage.clone(),
            )
            .unwrap(),
            path,
        )
    }
}

impl Drop for TestWallet {
    fn drop(&mut self) {
        if let Err(_) = std::fs::remove_dir_all(self.1.clone()) {
            println!("Couldn't remove wallet dir.")
        };
    }
}

impl Drop for TestSuite {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).expect("Couldn't remove wallet dir");
    }
}

pub fn wait_for_offer_is_stored(contract_id: ContractId, storage: Arc<MemoryStorage>) {
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

/// Helper function that reads `[bitcoin::bip32::Xpriv]` bytes from a file.
/// If the file does not exist then it will create a file `seed.ddk` in the specified path.
pub fn xprv_from_path(path: PathBuf, network: Network) -> anyhow::Result<Xpriv> {
    let seed_path = path.join("seed.ddk");
    let seed = if Path::new(&seed_path).exists() {
        let seed = std::fs::read(&seed_path)?;
        let mut key = [0; 32];
        key.copy_from_slice(&seed);
        let xprv = Xpriv::new_master(network, &seed)?;
        xprv
    } else {
        let mut file = File::create(&seed_path)?;
        let mut entropy = [0u8; 32];
        entropy.try_fill(&mut bitcoin::key::rand::thread_rng())?;
        // let _mnemonic = Mnemonic::from_entropy(&entropy)?;
        let xprv = Xpriv::new_master(network, &entropy)?;
        file.write_all(&entropy)?;
        xprv
    };

    Ok(seed)
}

pub fn memory_oracle() -> Oracle<KormirMemoryStorage> {
    let mut seed: [u8; 64] = [0; 64];
    bitcoin::key::rand::thread_rng().fill(&mut seed);
    let xpriv = Xpriv::new_master(Network::Regtest, &seed).unwrap();
    Oracle::from_xpriv(KormirMemoryStorage::default(), xpriv).unwrap()
}

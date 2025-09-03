#![allow(dead_code)]
use bitcoin::{
    address::NetworkChecked,
    bip32::Xpriv,
    key::{rand::Fill, Secp256k1},
    secp256k1::All,
    Address, Amount, Network,
};
use ddk::logger::{log_info, Logger, WriteLog};
use ddk_manager::{ContractId, Storage};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use bitcoincore_rpc::{Client, RpcApi};
use ddk::{
    builder::{Builder, SeedConfig},
    oracle::memory::MemoryOracle,
    storage::memory::MemoryStorage,
    transport::memory::MemoryTransport,
    DlcDevKit,
};

type TestDlcDevKit = DlcDevKit<MemoryTransport, MemoryStorage, MemoryOracle>;

pub async fn test_ddk(
    logger_one: Arc<Logger>,
    logger_two: Arc<Logger>,
) -> (TestSuite, TestSuite, Arc<MemoryOracle>) {
    let secp = Secp256k1::new();
    let oracle = Arc::new(MemoryOracle::default());

    let test = TestSuite::new(&secp, "send_offer", oracle.clone(), logger_one).await;
    let test_two = TestSuite::new(&secp, "sender_offer_two", oracle.clone(), logger_two).await;

    let node_one_address = test
        .ddk
        .wallet
        .new_external_address()
        .await
        .unwrap()
        .address;
    let node_two_address = test_two
        .ddk
        .wallet
        .new_external_address()
        .await
        .unwrap()
        .address;

    fund_addresses(&node_one_address, &node_two_address);

    test.ddk.wallet.sync().await.unwrap();
    test_two.ddk.wallet.sync().await.unwrap();

    (test, test_two, oracle)
}

pub fn get_bitcoind_client() -> Client {
    let bitcoind_user = std::env::var("BITCOIND_USER").expect("BITCOIND_USER must be set");
    let bitcoind_pass = std::env::var("BITCOIND_PASS").expect("BITCOIND_PASS must be set");
    let bitcoind_host = std::env::var("BITCOIND_HOST").expect("BITCOIND_HOST must be set");
    let auth = bitcoincore_rpc::Auth::UserPass(bitcoind_user, bitcoind_pass);
    bitcoincore_rpc::Client::new(&bitcoind_host, auth).unwrap()
}

pub fn fund_addresses(
    node_one_address: &Address<NetworkChecked>,
    node_two_address: &Address<NetworkChecked>,
) {
    let client = get_bitcoind_client();
    client
        .send_to_address(
            node_one_address,
            Amount::from_btc(1.1).unwrap(),
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
            Amount::from_btc(1.1).unwrap(),
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
    let client = get_bitcoind_client();
    let previous_height = client.get_block_count().unwrap();

    let address = client.get_new_address(None, None).unwrap().assume_checked();
    client.generate_to_address(num, &address).unwrap();
    let mut cur_block_height = previous_height;
    while cur_block_height < previous_height + num {
        sleep(Duration::from_secs(5));
        cur_block_height = client.get_block_count().unwrap();
    }
}

pub struct TestSuite {
    pub ddk: TestDlcDevKit,
}

impl TestSuite {
    pub async fn new(
        secp: &Secp256k1<All>,
        name: &str,
        oracle: Arc<MemoryOracle>,
        logger: Arc<Logger>,
    ) -> TestSuite {
        let mut seed = [0u8; 64];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let esplora_host = std::env::var("ESPLORA_HOST").expect("ESPLORA_HOST must be set");

        let transport = Arc::new(MemoryTransport::new(secp, logger.clone()));
        let storage = Arc::new(MemoryStorage::new());

        let mut builder = Builder::new();
        builder.set_network(Network::Regtest);
        builder.set_seed_bytes(SeedConfig::Bytes(seed)).unwrap();
        builder.set_esplora_host(esplora_host);
        builder.set_name(name);
        builder.set_oracle(oracle);
        builder.set_transport(transport);
        builder.set_storage(storage);
        builder.set_logger(logger);

        let ddk: TestDlcDevKit = builder.finish().await.unwrap();
        log_info!(ddk.logger.clone(), "DDK created for {}", name);

        TestSuite { ddk }
    }
}

pub async fn wait_for_offer_is_stored(contract_id: ContractId, storage: Arc<MemoryStorage>) {
    let mut tries = 0;
    let mut time = Duration::from_secs(1);
    loop {
        if tries == 5 {
            panic!("Never found contract.");
        }
        let offers = storage.get_contract_offers().await.unwrap();
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
pub fn xprv_from_path(path: PathBuf, network: Network) -> Xpriv {
    let seed_path = path.join("seed.ddk");
    if Path::new(&seed_path).exists() {
        let seed = std::fs::read(&seed_path).unwrap();
        let mut key = [0; 32];
        key.copy_from_slice(&seed);
        let xprv = Xpriv::new_master(network, &seed).unwrap();
        xprv
    } else {
        let mut file = File::create(&seed_path).unwrap();
        let mut entropy = [0u8; 32];
        entropy
            .try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let xprv = Xpriv::new_master(network, &entropy).unwrap();
        file.write_all(&entropy).unwrap();
        xprv
    }
}

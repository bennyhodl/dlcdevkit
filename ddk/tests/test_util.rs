#![allow(dead_code)]
use bitcoin::{
    address::NetworkChecked,
    bip32::Xpriv,
    key::{rand::Fill, Secp256k1},
    secp256k1::All,
    Address, Amount, Network,
};
use ddk_manager::{ContractId, Storage};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use bitcoincore_rpc::RpcApi;
use ddk::{
    builder::Builder, oracle::memory::MemoryOracle, storage::memory::MemoryStorage,
    transport::memory::MemoryTransport, DlcDevKit,
};

type TestDlcDevKit = DlcDevKit<MemoryTransport, MemoryStorage, MemoryOracle>;

pub async fn test_ddk() -> (TestSuite, TestSuite, Arc<MemoryOracle>) {
    let secp = Secp256k1::new();
    let oracle = Arc::new(MemoryOracle::default());

    let test = TestSuite::new(&secp, "send_offer", oracle.clone()).await;
    let test_two = TestSuite::new(&secp, "sender_offer_two", oracle.clone()).await;

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

pub fn fund_addresses(
    node_one_address: &Address<NetworkChecked>,
    node_two_address: &Address<NetworkChecked>,
) {
    let auth = bitcoincore_rpc::Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let client = bitcoincore_rpc::Client::new("http://127.0.0.1:18443", auth).unwrap();
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

pub struct TestSuite {
    pub ddk: TestDlcDevKit,
}

impl TestSuite {
    pub async fn new(secp: &Secp256k1<All>, name: &str, oracle: Arc<MemoryOracle>) -> TestSuite {
        let mut seed = [0u8; 32];
        seed.try_fill(&mut bitcoin::key::rand::thread_rng())
            .unwrap();
        let esplora_host = "http://127.0.0.1:30000".to_string();

        let transport = Arc::new(MemoryTransport::new(secp));
        let storage = Arc::new(MemoryStorage::new());

        let ddk: TestDlcDevKit = Builder::new()
            .set_network(Network::Regtest)
            .set_seed_bytes(seed)
            .set_esplora_host(esplora_host)
            .set_name(name)
            .set_oracle(oracle)
            .set_transport(transport)
            .set_storage(storage)
            .finish()
            .await
            .unwrap();

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

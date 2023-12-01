use crate::{ErnestWallet, Network, ernest::Ernest};
use core::time::Duration;
use electrsd::{
    bitcoind::{
        bitcoincore_rpc::{bitcoincore_rpc_json::AddressType, RpcApi},
        BitcoinD,
    },
    electrum_client::ElectrumApi,
    ElectrsD,
};
use std::sync::Arc;

pub fn setup_bitcoind_and_electrsd_and_ernest_wallet() -> (BitcoinD, ElectrsD, Arc<ErnestWallet>) {
    let bitcoind = electrsd::bitcoind::downloaded_exe_path().expect("No link?");
    let mut bitcoind_conf = electrsd::bitcoind::Conf::default();
    bitcoind_conf.network = "regtest";
    let bitcoind = BitcoinD::with_conf(bitcoind, &bitcoind_conf).unwrap();

    let electrs_exe = electrsd::downloaded_exe_path()
        .expect("you need to provide env var ELECTRS_EXE or specify an electrsd version feature");
    let mut electrsd_conf = electrsd::Conf::default();
    electrsd_conf.http_enabled = true;
    electrsd_conf.network = "regtest";
    let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &electrsd_conf).unwrap();

    let esplora_url = format!("http://{}", electrsd.esplora_url.as_ref().unwrap());

    let wallet = Arc::new(
        ErnestWallet::new(
            "test_wallet".to_string(),
            esplora_url.to_string(),
            Network::Regtest,
        )
        .unwrap(),
    );

    (bitcoind, electrsd, wallet)
}

pub fn setup_bitcoind_and_electrsd_and_ernest_manager() -> (BitcoinD, ElectrsD, Ernest) {
    let bitcoind = electrsd::bitcoind::downloaded_exe_path().expect("No link?");
    let mut bitcoind_conf = electrsd::bitcoind::Conf::default();
    bitcoind_conf.network = "regtest";
    let bitcoind = BitcoinD::with_conf(bitcoind, &bitcoind_conf).unwrap();

    let electrs_exe = electrsd::downloaded_exe_path()
        .expect("you need to provide env var ELECTRS_EXE or specify an electrsd version feature");
    let mut electrsd_conf = electrsd::Conf::default();
    electrsd_conf.http_enabled = true;
    electrsd_conf.network = "regtest";
    let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &electrsd_conf).unwrap();

    let esplora_url = format!("http://{}", electrsd.esplora_url.as_ref().unwrap());

    let manager = Ernest::new("test".to_string(), esplora_url.to_string(), Network::Regtest).unwrap(); 

    (bitcoind, electrsd, manager)
}

pub fn generate_blocks_and_wait(bitcoind: &BitcoinD, electrsd: &ElectrsD, num: usize) {
    print!("Generating {} blocks...", num);
    let cur_height = bitcoind
        .client
        .get_block_count()
        .expect("failed to get current block height");
    let address = bitcoind
        .client
        .get_new_address(Some("test"), Some(AddressType::Legacy))
        .expect("failed to get new address");
    // TODO: expect this Result once the WouldBlock issue is resolved upstream.
    let _block_hashes_res = bitcoind.client.generate_to_address(num as u64, &address);
    wait_for_block(electrsd, cur_height as usize + num);
    print!(" Done!");
    println!("\n");
}

pub fn wait_for_block(electrsd: &ElectrsD, min_height: usize) {
    let mut header = match electrsd.client.block_headers_subscribe() {
        Ok(header) => header,
        Err(_) => {
            // While subscribing should succeed the first time around, we ran into some cases where
            // it didn't. Since we can't proceed without subscribing, we try again after a delay
            // and panic if it still fails.
            std::thread::sleep(Duration::from_secs(1));
            electrsd
                .client
                .block_headers_subscribe()
                .expect("failed to subscribe to block headers")
        }
    };
    loop {
        if header.height >= min_height {
            break;
        }
        header = exponential_backoff_poll(|| {
            electrsd.trigger().expect("failed to trigger electrsd");
            electrsd.client.ping().expect("failed to ping electrsd");
            electrsd
                .client
                .block_headers_pop()
                .expect("failed to pop block header")
        });
    }
}

fn exponential_backoff_poll<T, F>(mut poll: F) -> T
where
    F: FnMut() -> Option<T>,
{
    let mut delay = Duration::from_millis(64);
    let mut tries = 0;
    loop {
        match poll() {
            Some(data) => break data,
            None if delay.as_millis() < 512 => {
                delay = delay.mul_f32(2.0);
            }

            None => {}
        }
        assert!(tries < 20, "Reached max tries.");
        tries += 1;
        std::thread::sleep(delay);
    }
}

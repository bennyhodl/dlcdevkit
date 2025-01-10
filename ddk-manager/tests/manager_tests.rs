#[macro_use]
#[allow(dead_code)]
mod test_utils;

use bitcoin::Network;
use ddk::chain::EsploraClient;
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk_manager::{manager::Manager, CachedContractSignerProvider, Oracle, SimpleSigner};
use dlc_messages::Message;
use secp256k1_zkp::{rand::Fill, PublicKey, XOnlyPublicKey};
use std::{collections::HashMap, sync::Arc};
use test_utils::{set_time, MockTime};

type TestManager = Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Arc<MemoryStorage>,
    Arc<MemoryOracle>,
    Arc<MockTime>,
    SimpleSigner,
>;

async fn get_manager() -> TestManager {
    let blockchain =
        Arc::new(EsploraClient::new("http://localhost:30000", Network::Regtest).unwrap());
    let store = Arc::new(MemoryStorage::new());
    let mut seed = [0u8; 32];
    seed.try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();
    let wallet = Arc::new(
        DlcDevKitWallet::new(
            "manager",
            &seed,
            "http://localhost:30000",
            Network::Regtest,
            store.clone(),
        )
        .unwrap(),
    );

    let oracle_list = (0..5).map(|_| MemoryOracle::default()).collect::<Vec<_>>();
    let oracles: HashMap<XOnlyPublicKey, _> = oracle_list
        .into_iter()
        .map(|x| (x.get_public_key(), Arc::new(x)))
        .collect();
    let time = Arc::new(MockTime {});

    set_time(0);

    Manager::new(
        wallet.clone(),
        wallet.clone(),
        blockchain.clone(),
        store.clone(),
        oracles,
        time,
    )
    .await
    .unwrap()
}

fn pubkey() -> PublicKey {
    "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
        .parse()
        .unwrap()
}

#[tokio::test]
async fn reject_offer_with_existing_contract_id() {
    let offer_message = Message::Offer(
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap(),
    );

    let manager = get_manager().await;

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect("To accept the first offer message");

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect_err("To reject the second offer message");
}

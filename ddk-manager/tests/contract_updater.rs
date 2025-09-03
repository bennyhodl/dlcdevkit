#[macro_use]
#[allow(dead_code)]
mod test_utils;

use std::sync::Arc;

use bitcoin::Network;
use ddk::chain::EsploraClient;
use ddk::logger::Logger;
use ddk_manager::contract::offered_contract::OfferedContract;
use secp256k1_zkp::rand::Fill;
use secp256k1_zkp::PublicKey;

#[tokio::test]
async fn accept_contract_test() {
    let logger = Arc::new(Logger::disabled("test_contract_updater".to_string()));
    let offer_dlc =
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap();
    let dummy_pubkey: PublicKey =
        "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443"
            .parse()
            .unwrap();
    let mut keys_id = [0u8; 32];
    keys_id
        .try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();
    let offered_contract =
        OfferedContract::try_from_offer_dlc(&offer_dlc, dummy_pubkey, keys_id).unwrap();
    let blockchain = Arc::new(
        EsploraClient::new("http://localhost:30000", Network::Regtest, logger.clone()).unwrap(),
    );

    let stuff = test_utils::create_and_fund_wallet(logger.clone(), blockchain.clone()).await;
    let wallet = Arc::new(stuff.0);
    wallet.sync().await.unwrap();

    ddk_manager::contract_updater::accept_contract(
        secp256k1_zkp::SECP256K1,
        &offered_contract,
        &wallet,
        &wallet,
        &blockchain,
        &logger,
    )
    .await
    .expect("Not to fail");
}

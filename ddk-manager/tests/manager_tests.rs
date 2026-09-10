#[macro_use]
#[allow(dead_code)]
mod test_utils;

use bitcoin::Network;
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk::{chain::EsploraClient, logger::Logger};
use ddk_manager::{manager::Manager, CachedContractSignerProvider, Oracle, SimpleSigner};
use ddk_messages::Message;
use ddk_testenv::dlc::{announce_enum_event, contract_input, enum_descriptor, ContractLeg};
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
    Arc<EsploraClient>,
    SimpleSigner,
    Arc<Logger>,
>;

async fn get_manager(logger: Arc<Logger>) -> TestManager {
    // These tests reject offers before anything reaches the chain and never
    // mine, so the backends shared across this binary are enough.
    let blockchain = Arc::new(
        EsploraClient::new(
            ddk_testenv::env().esplora_host(),
            Network::Regtest,
            logger.clone(),
        )
        .unwrap(),
    );
    let store = Arc::new(MemoryStorage::new());
    let mut seed = [0u8; 64];
    seed.try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();
    let wallet = Arc::new(
        DlcDevKitWallet::new(
            &seed,
            blockchain.clone(),
            Network::Regtest,
            store.clone(),
            None,
            logger.clone(),
        )
        .await
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
        blockchain,
        logger,
        None,
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
    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let offer_message = Message::Offer(
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap(),
    );

    let manager = get_manager(logger).await;

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect("To accept the first offer message");

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect_err("To reject the second offer message");
}

/// The fixture offer's oracle event matures at this unix time.
const FIXTURE_MATURITY: u64 = 1_623_133_104;

#[tokio::test]
async fn reject_offer_on_matured_event() {
    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let offer_message = Message::Offer(
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap(),
    );

    let manager = get_manager(logger).await;

    set_time(FIXTURE_MATURITY);
    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect_err("To reject an offer whose event has matured");

    set_time(FIXTURE_MATURITY - 1);
    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect("To accept an offer whose event is still in the future");
}

#[tokio::test]
async fn reject_accept_on_matured_event() {
    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let offer: ddk_messages::OfferDlc =
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap();
    let contract_id = offer.temporary_contract_id;

    let manager = get_manager(logger).await;

    set_time(FIXTURE_MATURITY - 1);
    manager
        .on_dlc_message(&Message::Offer(offer), pubkey())
        .await
        .expect("To accept an offer whose event is still in the future");

    set_time(FIXTURE_MATURITY);
    manager
        .accept_contract_offer(&contract_id)
        .await
        .expect_err("To reject accepting an offer whose event matured while it waited");
}

#[tokio::test]
async fn reject_offer_creation_with_mismatched_announcements() {
    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let manager = get_manager(logger).await;

    let oracles = vec![MemoryOracle::default(), MemoryOracle::default()];
    let announcements = announce_enum_event(&oracles, "mismatch", 2_000_000_000).await;
    let total = bitcoin::Amount::from_sat(100_000);
    let two_legs = contract_input(
        &[
            ContractLeg::new(enum_descriptor(total), vec![announcements[0].clone()], 1),
            ContractLeg::new(enum_descriptor(total), vec![announcements[1].clone()], 1),
        ],
        total,
        bitcoin::Amount::ZERO,
        2,
    );

    // One announcement list for two contract infos used to hit an assert.
    let result = manager
        .send_offer_with_announcements(&two_legs, pubkey(), vec![announcements])
        .await;
    assert!(
        matches!(result, Err(ddk_manager::error::Error::InvalidParameters(_))),
        "unexpected result: {result:?}"
    );
}

#[tokio::test]
async fn reject_channel_offer_with_existing_channel_id() {
    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let offer_message = Message::OfferChannel(
        serde_json::from_str(include_str!("../test_inputs/offer_channel.json")).unwrap(),
    );

    let manager = get_manager(logger).await;

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect("To accept the first offer message");

    manager
        .on_dlc_message(&offer_message, pubkey())
        .await
        .expect_err("To reject the second offer message");
}

#[tokio::test]
async fn commit_offer_stores_the_message_stream() {
    use ddk_manager::contract::Contract;
    use ddk_manager::Storage;

    let logger = Arc::new(Logger::disabled("test_manager".to_string()));
    let mut offer: ddk_messages::OfferDlc =
        serde_json::from_str(include_str!("../test_inputs/offer_contract.json")).unwrap();

    let manager = get_manager(logger).await;
    manager
        .on_dlc_message(&Message::Offer(offer.clone()), pubkey())
        .await
        .unwrap();

    // A record of type 65007 with a one-byte body.
    let record_bytes = [0xfd, 0xfd, 0xef, 0x01, 7];
    offer.tlvs = ddk_messages::tlv_stream::TlvStream::read_to_end(
        &mut ddk_messages::lightning::io::Cursor::new(record_bytes),
    )
    .unwrap();
    manager.commit_offer(&offer).await.unwrap();

    let contract = manager
        .get_store()
        .get_contract(&offer.temporary_contract_id)
        .await
        .unwrap()
        .unwrap();
    let Contract::Offered(offered) = contract else {
        panic!("contract is not offered")
    };
    assert_eq!(offered.tlvs, offer.tlvs);
}

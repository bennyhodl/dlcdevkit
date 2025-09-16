use bitcoin::Amount;
use bitcoincore_rpc::RpcApi;
use ddk::logger::LogLevel;
use ddk::{chain::EsploraClient, logger::Logger, oracle::memory::MemoryOracle};
use ddk_dlc::{EnumerationPayout, Payout};
use ddk_manager::contract::Contract;
use ddk_manager::{
    contract::contract_input::{ContractInputInfo, OracleInput},
    Oracle,
};
use ddk_manager::{
    contract::{
        contract_input::ContractInput, enum_descriptor::EnumDescriptor, ContractDescriptor,
    },
    manager::Manager,
    Storage,
};
use ddk_messages::Message;
use lightning::util::ser::Writeable;
use secp256k1_zkp::rand::RngCore;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::test_utils::{generate_blocks, EVENT_MATURITY};

mod test_utils;

const TOTAL_COLLATERAL: Amount = Amount::ONE_BTC;
const SPLICE_AMOUNT: Amount = Amount::from_sat(50_000_000);

#[derive(Debug, Clone)]
enum SplicePath {
    SpliceIn,
    SpliceOut,
}

async fn splice_execution_test(test_params: test_utils::TestParams) {
    let funding_collateral = TOTAL_COLLATERAL + Amount::from_sat(300);
    let logger = Arc::new(Logger::console(
        "splice_execution_tests".to_string(),
        LogLevel::Debug,
    ));
    let electrs_host = std::env::var("ESPLORA_HOST").expect("ESPLORA_HOST must be set");
    let electrs = Arc::new(
        EsploraClient::new(&electrs_host, bitcoin::Network::Regtest, logger.clone()).unwrap(),
    );

    let (alice_wallet, alice_storage, bob_wallet, bob_storage, sink_rpc) =
        test_utils::init_clients(
            logger.clone(),
            electrs.clone(),
            funding_collateral,
            Amount::ZERO,
        )
        .await;
    let alice_wallet = Arc::new(alice_wallet);
    let bob_wallet = Arc::new(bob_wallet);
    let sink = Arc::new(sink_rpc);

    let mut alice_oracles = HashMap::with_capacity(1);
    let mut bob_oracles = HashMap::with_capacity(1);

    for oracle in test_params.oracles.clone() {
        let oracle = Arc::new(oracle);
        alice_oracles.insert(oracle.get_public_key(), Arc::clone(&oracle));
        bob_oracles.insert(oracle.get_public_key(), Arc::clone(&oracle));
    }

    let mock_time = Arc::new(test_utils::MockTime {});
    // For splice tests, set time much earlier to keep original DLC far from maturity
    let initial_time = (test_utils::EVENT_MATURITY as u64) - 3600;

    test_utils::set_time(initial_time);

    test_utils::generate_blocks(6, electrs.clone(), sink.clone()).await;

    test_utils::refresh_wallet(&alice_wallet, funding_collateral.to_sat()).await;
    test_utils::refresh_wallet(&bob_wallet, Amount::ZERO.to_sat()).await;

    let alice_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&alice_wallet),
            Arc::clone(&alice_wallet),
            Arc::clone(&electrs),
            Arc::clone(&alice_storage),
            alice_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
            logger.clone(),
        )
        .await
        .unwrap(),
    ));

    let bob_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&bob_wallet),
            Arc::clone(&bob_wallet),
            Arc::clone(&electrs),
            Arc::clone(&bob_storage),
            bob_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
            logger.clone(),
        )
        .await
        .unwrap(),
    ));

    let public_key = "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
        .parse()
        .unwrap();

    let alice_offer_msg = alice_manager
        .lock()
        .await
        .send_offer(&test_params.contract_input, public_key)
        .await
        .unwrap();

    bob_manager
        .lock()
        .await
        .on_dlc_message(&Message::Offer(alice_offer_msg.clone()), public_key)
        .await
        .unwrap();

    let (original_contract_id, _, bob_accept_msg) = bob_manager
        .lock()
        .await
        .accept_contract_offer(&alice_offer_msg.temporary_contract_id)
        .await
        .unwrap();

    let alice_sign_msg = alice_manager
        .lock()
        .await
        .on_dlc_message(&Message::Accept(bob_accept_msg.clone()), public_key)
        .await
        .unwrap();

    let Message::Sign(sign_msg) = alice_sign_msg.unwrap() else {
        panic!("Alice did not sign the contract");
    };

    bob_manager
        .lock()
        .await
        .on_dlc_message(&Message::Sign(sign_msg), public_key)
        .await
        .unwrap();

    alice_manager
        .lock()
        .await
        .periodic_check(false)
        .await
        .unwrap();
    bob_manager
        .lock()
        .await
        .periodic_check(false)
        .await
        .unwrap();

    let Contract::Signed(signed_contract) = bob_manager
        .lock()
        .await
        .get_store()
        .get_contract(&original_contract_id)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("Original contract is not signed");
    };
    let original_funding_txid = signed_contract
        .accepted_contract
        .dlc_transactions
        .fund
        .compute_txid();

    periodic_check!(alice_manager, original_contract_id, Signed);
    periodic_check!(bob_manager, original_contract_id, Signed);
    generate_blocks(10, electrs.clone(), sink.clone()).await;
    periodic_check!(alice_manager, original_contract_id, Confirmed);
    periodic_check!(bob_manager, original_contract_id, Confirmed);

    // Assert that funding txid is mined
    let confirmations = electrs
        .async_client
        .get_tx_status(&original_funding_txid)
        .await
        .unwrap();
    assert!(confirmations.confirmed);

    let splice_path = if bitcoin::key::rand::thread_rng().next_u32() % 2 == 0 {
        SplicePath::SpliceIn
    } else {
        SplicePath::SpliceOut
    };

    let contract_input =
        get_splice_test_params(test_params.oracles[0].clone(), splice_path.clone()).await;

    match splice_path {
        SplicePath::SpliceIn => {
            let send_splice_funds = alice_wallet.new_external_address().await.unwrap().address;
            sink.send_to_address(
                &send_splice_funds,
                SPLICE_AMOUNT + Amount::from_sat(492),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            generate_blocks(5, electrs.clone(), sink.clone()).await;
            alice_wallet.sync().await.unwrap();
            let balance = alice_wallet.get_balance().await.unwrap();
            assert!(balance.confirmed == SPLICE_AMOUNT + Amount::from_sat(492));
        }
        SplicePath::SpliceOut => {}
    }

    let alice_splice_offer_msg = alice_manager
        .lock()
        .await
        .send_splice_offer(&contract_input, public_key, &original_contract_id)
        .await
        .unwrap();

    bob_manager
        .lock()
        .await
        .on_dlc_message(&Message::Offer(alice_splice_offer_msg.clone()), public_key)
        .await
        .unwrap();

    let (splice_contract_id, _, bob_splice_accept_msg) = bob_manager
        .lock()
        .await
        .accept_contract_offer(&alice_splice_offer_msg.temporary_contract_id)
        .await
        .unwrap();

    let alice_splice_sign_msg = alice_manager
        .lock()
        .await
        .on_dlc_message(&Message::Accept(bob_splice_accept_msg.clone()), public_key)
        .await
        .unwrap();

    let Message::Sign(sign_msg) = alice_splice_sign_msg.unwrap() else {
        panic!("Alice did not sign the splice contract");
    };

    bob_manager
        .lock()
        .await
        .on_dlc_message(&Message::Sign(sign_msg), public_key)
        .await
        .unwrap();

    periodic_check!(alice_manager, splice_contract_id, Signed);
    periodic_check!(bob_manager, splice_contract_id, Signed);
    periodic_check!(bob_manager, original_contract_id, PreClosed);
    periodic_check!(alice_manager, original_contract_id, PreClosed);
    let Contract::Signed(spliced_signed_contract) = alice_manager
        .lock()
        .await
        .get_store()
        .get_contract(&splice_contract_id)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("Original contract is not signed");
    };

    generate_blocks(10, electrs.clone(), sink.clone()).await;
    periodic_check!(alice_manager, splice_contract_id, Confirmed);
    periodic_check!(bob_manager, splice_contract_id, Confirmed);
    periodic_check!(bob_manager, original_contract_id, Closed);
    periodic_check!(alice_manager, original_contract_id, Closed);

    let splice_funding_transaction = spliced_signed_contract
        .accepted_contract
        .dlc_transactions
        .fund;
    assert!(splice_funding_transaction
        .input
        .iter()
        .find(|i| i.previous_output.txid == original_funding_txid)
        .is_some());

    let dlc_input = spliced_signed_contract
        .accepted_contract
        .offered_contract
        .funding_inputs
        .iter()
        .find(|i| i.dlc_input.is_some())
        .unwrap()
        .dlc_input
        .as_ref()
        .unwrap();
    assert_eq!(dlc_input.contract_id, original_contract_id);
    match splice_path {
        SplicePath::SpliceIn => {
            println!(
                "Splice in funding transaction output value: {:?}",
                splice_funding_transaction.output[0].value
            );
            assert!(splice_funding_transaction.output[0].value > TOTAL_COLLATERAL);
        }
        SplicePath::SpliceOut => {
            println!(
                "Splice out funding transaction output value: {:?}",
                splice_funding_transaction.output[0].value
            );
            assert!(splice_funding_transaction.output[0].value < TOTAL_COLLATERAL);
        }
    }

    let outcome = if bitcoin::key::rand::thread_rng().next_u32() % 2 == 0 {
        "REPAID".to_string()
    } else {
        "NOT_REPAID".to_string()
    };
    let attestation = test_params.oracles[0]
        .oracle
        .sign_enum_event("SPLICE_CONTRACT".to_string(), outcome.clone())
        .await
        .unwrap();
    assert!(attestation.outcomes.contains(&outcome));
    test_utils::set_time(EVENT_MATURITY as u64 + 5);
    periodic_check!(alice_manager, splice_contract_id, PreClosed);
    periodic_check!(bob_manager, splice_contract_id, PreClosed);
    periodic_check!(bob_manager, original_contract_id, Closed);
    periodic_check!(alice_manager, original_contract_id, Closed);
    generate_blocks(10, electrs.clone(), sink.clone()).await;
    periodic_check!(alice_manager, splice_contract_id, Closed);
    periodic_check!(bob_manager, splice_contract_id, Closed);
    periodic_check!(bob_manager, original_contract_id, Closed);
    periodic_check!(alice_manager, original_contract_id, Closed);

    let Contract::Closed(closed_splice_contract) = alice_manager
        .lock()
        .await
        .get_store()
        .get_contract(&splice_contract_id)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("Splice contract is not closed");
    };

    let closed_cet = closed_splice_contract.signed_cet.unwrap();
    let contains_original_funding_txid = closed_cet
        .input
        .iter()
        .find(|i| i.previous_output.txid == splice_funding_transaction.compute_txid())
        .is_some();
    assert!(contains_original_funding_txid);

    let confirmations = electrs
        .async_client
        .get_tx_status(&closed_cet.compute_txid())
        .await
        .unwrap();
    assert!(confirmations.confirmed);

    if &outcome == "REPAID" {
        let payout_address = closed_cet.output[0].script_pubkey.clone();
        let contract_payout_address = spliced_signed_contract
            .accepted_contract
            .offered_contract
            .offer_params
            .payout_script_pubkey;
        assert_eq!(payout_address, contract_payout_address);
    } else {
        let payout_address = closed_cet.output[0].script_pubkey.clone();
        let contract_payout_address = spliced_signed_contract
            .accepted_contract
            .accept_params
            .payout_script_pubkey;
        assert_eq!(payout_address, contract_payout_address);
    }
}

async fn splice_test_params() -> test_utils::TestParams {
    let oracle = MemoryOracle::default();
    let announcement = oracle
        .oracle
        .create_enum_event(
            "SPlICE".to_string(),
            vec!["REPAID".to_string(), "NOT_REPAID".to_string()],
            test_utils::EVENT_MATURITY,
        )
        .await
        .unwrap();
    let contract_descriptor = ContractDescriptor::Enum(EnumDescriptor {
        outcome_payouts: vec![
            EnumerationPayout {
                outcome: "REPAID".to_string(),
                payout: Payout {
                    offer: TOTAL_COLLATERAL,
                    accept: Amount::ZERO,
                },
            },
            EnumerationPayout {
                outcome: "NOT_REPAID".to_string(),
                payout: Payout {
                    offer: Amount::ZERO,
                    accept: TOTAL_COLLATERAL,
                },
            },
        ],
    });
    let contract_input_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: vec![oracle.get_public_key()],
            event_id: announcement.oracle_event.event_id,
            threshold: 1,
        },
    };
    let contract_input = ContractInput {
        offer_collateral: TOTAL_COLLATERAL,
        accept_collateral: Amount::ZERO,
        fee_rate: 1,
        contract_infos: vec![contract_input_info],
    };
    test_utils::TestParams {
        oracles: vec![oracle],
        contract_input,
    }
}

async fn get_splice_test_params(oracle: MemoryOracle, splice_path: SplicePath) -> ContractInput {
    let announcement = oracle
        .oracle
        .create_enum_event(
            "SPLICE_CONTRACT".to_string(),
            vec!["REPAID".to_string(), "NOT_REPAID".to_string()],
            test_utils::EVENT_MATURITY,
        )
        .await
        .unwrap();
    let amount = match splice_path {
        SplicePath::SpliceIn => TOTAL_COLLATERAL + SPLICE_AMOUNT,
        SplicePath::SpliceOut => TOTAL_COLLATERAL - SPLICE_AMOUNT,
    };
    let contract_descriptor = ContractDescriptor::Enum(EnumDescriptor {
        outcome_payouts: vec![
            EnumerationPayout {
                outcome: "REPAID".to_string(),
                payout: Payout {
                    offer: amount,
                    accept: Amount::ZERO,
                },
            },
            EnumerationPayout {
                outcome: "NOT_REPAID".to_string(),
                payout: Payout {
                    offer: Amount::ZERO,
                    accept: amount,
                },
            },
        ],
    });
    let contract_input_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: vec![announcement.oracle_public_key],
            event_id: announcement.oracle_event.event_id,
            threshold: 1,
        },
    };
    let contract_input = ContractInput {
        offer_collateral: amount,
        accept_collateral: Amount::ZERO,
        fee_rate: 1,
        contract_infos: vec![contract_input_info],
    };

    contract_input
}

#[tokio::test]
#[ignore]
async fn splice() {
    dotenv::dotenv().ok();
    splice_execution_test(splice_test_params().await).await
}

#[macro_use]
#[allow(dead_code)]
mod test_utils;

use ddk::chain::EsploraClient;
use ddk_manager::payout_curve::PayoutFunctionPiece;
use test_utils::*;

use bitcoincore_rpc::RpcApi;
use ddk_manager::contract::{numerical_descriptor::DifferenceParams, Contract};
use ddk_manager::manager::Manager;
use ddk_manager::{Blockchain, Oracle, Storage};
use dlc_messages::oracle_msgs::OracleAttestation;
use dlc_messages::{AcceptDlc, OfferDlc, SignDlc};
use dlc_messages::{CetAdaptorSignatures, Message};
use lightning::ln::wire::Type;
use lightning::util::ser::Writeable;
use secp256k1_zkp::rand::{thread_rng, RngCore};
use secp256k1_zkp::{ecdsa::Signature, EcdsaAdaptorSignature};
use serde_json::from_str;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use test_utils::init_clients;
use tokio::sync::mpsc::channel;
use tokio::sync::Mutex;
use tokio::time::sleep;
#[derive(serde::Serialize, serde::Deserialize)]
struct TestVectorPart<T> {
    message: T,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "dlc_messages::serde_utils::serialize_hex",
            deserialize_with = "dlc_messages::serde_utils::deserialize_hex_string"
        )
    )]
    serialized: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TestVector {
    offer_message: TestVectorPart<OfferDlc>,
    accept_message: TestVectorPart<AcceptDlc>,
    sign_message: TestVectorPart<SignDlc>,
}

fn write_message<T: Writeable + serde::Serialize + Type>(_msg_name: &str, s: T) {
    if std::env::var("GENERATE_TEST_VECTOR").is_ok() {
        let mut buf = Vec::new();
        s.type_id().write(&mut buf).unwrap();
        s.write(&mut buf).unwrap();
        let _t = TestVectorPart {
            message: s,
            serialized: buf,
        };
        // to_writer_pretty(
        //     &std::fs::File::create(format!("{}.json", msg_name)).unwrap(),
        //     &t,
        // )
        // .unwrap();
    }
}

async fn create_test_vector() {
    if std::env::var("GENERATE_TEST_VECTOR").is_ok() {
        let _test_vector = TestVector {
            offer_message: from_str(
                &tokio::fs::read_to_string("offer_message.json")
                    .await
                    .unwrap(),
            )
            .unwrap(),
            accept_message: from_str(
                &tokio::fs::read_to_string("accept_message.json")
                    .await
                    .unwrap(),
            )
            .unwrap(),
            sign_message: from_str(
                &tokio::fs::read_to_string("sign_message.json")
                    .await
                    .unwrap(),
            )
            .unwrap(),
        };
        let _file_name = std::env::var("TEST_VECTOR_OUTPUT_NAME")
            .unwrap_or_else(|_| "test_vector.json".to_string());
        // to_writer_pretty(std::fs::File::create(file_name).unwrap(), &test_vector).unwrap();
    }
}

macro_rules! periodic_check {
    ($d:expr, $id:expr, $p:ident) => {
        $d.lock()
            .await
            .periodic_check(true)
            .await
            .expect("Periodic check error");
        assert_contract_state!($d, $id, $p);
    };
}

async fn numerical_common<F>(
    nb_oracles: usize,
    threshold: usize,
    payout_function_pieces_cb: F,
    difference_params: Option<DifferenceParams>,
    manual_close: bool,
) where
    F: Fn(usize) -> Vec<PayoutFunctionPiece>,
{
    let oracle_numeric_infos = get_same_num_digits_oracle_numeric_infos(nb_oracles);
    let with_diff = difference_params.is_some();
    let contract_descriptor = get_numerical_contract_descriptor(
        oracle_numeric_infos.clone(),
        payout_function_pieces_cb(*oracle_numeric_infos.nb_digits.iter().min().unwrap()),
        difference_params,
    );
    manager_execution_test(
        get_numerical_test_params(
            &oracle_numeric_infos,
            threshold,
            with_diff,
            contract_descriptor,
            false,
        )
        .await,
        TestPath::Close,
        manual_close,
    )
    .await;
}

async fn numerical_polynomial_common(
    nb_oracles: usize,
    threshold: usize,
    difference_params: Option<DifferenceParams>,
    manual_close: bool,
) {
    numerical_common(
        nb_oracles,
        threshold,
        get_polynomial_payout_curve_pieces,
        difference_params,
        manual_close,
    )
    .await;
}

async fn numerical_common_diff_nb_digits(
    nb_oracles: usize,
    threshold: usize,
    difference_params: Option<DifferenceParams>,
    use_max_value: bool,
    manual_close: bool,
) {
    let with_diff = difference_params.is_some();
    let oracle_numeric_infos = get_variable_oracle_numeric_infos(
        &(0..nb_oracles)
            .map(|_| (NB_DIGITS + (thread_rng().next_u32() % 6)) as usize)
            .collect::<Vec<_>>(),
    );
    let contract_descriptor = get_numerical_contract_descriptor(
        oracle_numeric_infos.clone(),
        get_polynomial_payout_curve_pieces(oracle_numeric_infos.get_min_nb_digits()),
        difference_params,
    );

    manager_execution_test(
        get_numerical_test_params(
            &oracle_numeric_infos,
            threshold,
            with_diff,
            contract_descriptor,
            use_max_value,
        )
        .await,
        TestPath::Close,
        manual_close,
    )
    .await;
}

#[derive(Eq, PartialEq, Clone)]
enum TestPath {
    Close,
    Refund,
    BadAcceptCetSignature,
    BadAcceptRefundSignature,
    BadSignCetSignature,
    BadSignRefundSignature,
}

#[tokio::test]
#[ignore]
async fn single_oracle_numerical_test() {
    numerical_polynomial_common(1, 1, None, false).await;
}

#[tokio::test]
#[ignore]
async fn single_oracle_numerical_manual_test() {
    numerical_polynomial_common(1, 1, None, true).await;
}

#[tokio::test]
#[ignore]
async fn single_oracle_numerical_hyperbola_test() {
    numerical_common(1, 1, get_hyperbola_payout_curve_pieces, None, false).await;
}

#[tokio::test]
#[ignore]
async fn three_of_three_oracle_numerical_test() {
    numerical_polynomial_common(3, 3, None, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_test() {
    numerical_polynomial_common(5, 2, None, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_manual_test() {
    numerical_polynomial_common(5, 2, None, true).await;
}

#[tokio::test]
#[ignore]
async fn three_of_three_oracle_numerical_with_diff_test() {
    numerical_polynomial_common(3, 3, Some(get_difference_params()), false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_with_diff_test() {
    numerical_polynomial_common(5, 2, Some(get_difference_params()), false).await;
}

#[tokio::test]
#[ignore]
async fn three_of_five_oracle_numerical_with_diff_test() {
    numerical_polynomial_common(5, 3, Some(get_difference_params()), false).await;
}

#[tokio::test]
#[ignore]
async fn three_of_five_oracle_numerical_with_diff_manual_test() {
    numerical_polynomial_common(5, 3, Some(get_difference_params()), true).await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_manual_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_3_of_3_test() {
    manager_execution_test(
        get_enum_test_params(3, 3, None).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_3_of_3_manual_test() {
    manager_execution_test(
        get_enum_test_params(3, 3, None).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_3_of_5_test() {
    manager_execution_test(
        get_enum_test_params(5, 3, None).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_3_of_5_manual_test() {
    manager_execution_test(
        get_enum_test_params(5, 3, None).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_with_diff_3_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, true, Some(get_difference_params())).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_with_diff_3_of_5_manual_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, true, Some(get_difference_params())).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_with_diff_5_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 5, true, Some(get_difference_params())).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_with_diff_5_of_5_manual_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 5, true, Some(get_difference_params())).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_3_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, false, None).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_3_of_5_manual_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, false, None).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_5_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 5, false, None).await,
        TestPath::Close,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_and_numerical_5_of_5_manual_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 5, false, None).await,
        TestPath::Close,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_refund_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::Refund,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_refund_manual_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::Refund,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_bad_accept_cet_sig_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::BadAcceptCetSignature,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_bad_accept_refund_sig_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::BadAcceptRefundSignature,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_bad_sign_cet_sig_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::BadSignCetSignature,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn enum_single_oracle_bad_sign_refund_sig_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::BadSignRefundSignature,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn two_of_two_oracle_numerical_diff_nb_digits_test() {
    numerical_common_diff_nb_digits(2, 2, None, false, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_two_oracle_numerical_diff_nb_digits_manual_test() {
    numerical_common_diff_nb_digits(2, 2, None, false, true).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_diff_nb_digits_test() {
    numerical_common_diff_nb_digits(5, 2, None, false, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_diff_nb_digits_manual_test() {
    numerical_common_diff_nb_digits(5, 2, None, false, true).await;
}

#[tokio::test]
#[ignore]
async fn two_of_two_oracle_numerical_with_diff_diff_nb_digits_test() {
    numerical_common_diff_nb_digits(2, 2, Some(get_difference_params()), false, false).await;
}

#[tokio::test]
#[ignore]
async fn three_of_three_oracle_numerical_with_diff_diff_nb_digits_test() {
    numerical_common_diff_nb_digits(3, 3, Some(get_difference_params()), false, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_with_diff_diff_nb_digits_test() {
    numerical_common_diff_nb_digits(5, 2, Some(get_difference_params()), false, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_two_oracle_numerical_with_diff_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(2, 2, Some(get_difference_params()), true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_three_oracle_numerical_with_diff_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(3, 2, Some(get_difference_params()), true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_with_diff_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(5, 2, Some(get_difference_params()), true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_with_diff_diff_nb_digits_max_value_manual_test() {
    numerical_common_diff_nb_digits(5, 2, Some(get_difference_params()), true, true).await;
}

#[tokio::test]
#[ignore]
async fn two_of_two_oracle_numerical_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(2, 2, None, true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_three_oracle_numerical_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(3, 2, None, true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_diff_nb_digits_max_value_test() {
    numerical_common_diff_nb_digits(5, 2, None, true, false).await;
}

#[tokio::test]
#[ignore]
async fn two_of_five_oracle_numerical_diff_nb_digits_max_value_manual_test() {
    numerical_common_diff_nb_digits(5, 2, None, true, true).await;
}

fn alter_adaptor_sig(input: &mut CetAdaptorSignatures) {
    let sig_index = thread_rng().next_u32() as usize % input.ecdsa_adaptor_signatures.len();

    let mut copy = input.ecdsa_adaptor_signatures[sig_index]
        .signature
        .as_ref()
        .to_vec();
    let i = thread_rng().next_u32() as usize % secp256k1_zkp::ffi::ECDSA_ADAPTOR_SIGNATURE_LENGTH;
    copy[i] = copy[i].checked_add(1).unwrap_or(0);
    input.ecdsa_adaptor_signatures[sig_index].signature =
        EcdsaAdaptorSignature::from_slice(&copy).unwrap();
}

fn alter_refund_sig(refund_signature: &Signature) -> Signature {
    let mut copy = refund_signature.serialize_compact();
    let i = thread_rng().next_u32() as usize % secp256k1_zkp::constants::COMPACT_SIGNATURE_SIZE;
    copy[i] = copy[i].checked_add(1).unwrap_or(0);
    Signature::from_compact(&copy).unwrap()
}

async fn get_attestations(test_params: &TestParams) -> Vec<(usize, OracleAttestation)> {
    let mut attestations = Vec::new();
    for contract_info in test_params.contract_input.contract_infos.iter() {
        attestations.clear();
        for (i, pk) in contract_info.oracles.public_keys.iter().enumerate() {
            let oracle = test_params
                .oracles
                .iter()
                .find(|x| x.get_public_key() == *pk);
            if let Some(o) = oracle {
                if let Ok(attestation) = o.get_attestation(&contract_info.oracles.event_id).await {
                    attestations.push((i, attestation));
                }
            }
        }
        if attestations.len() >= contract_info.oracles.threshold as usize {
            return attestations;
        }
    }

    panic!("No attestations found");
}

async fn manager_execution_test(test_params: TestParams, path: TestPath, manual_close: bool) {
    env_logger::try_init().ok();
    let (alice_send, mut bob_receive) = channel::<Option<Message>>(100);
    let (bob_send, mut alice_receive) = channel::<Option<Message>>(100);
    let (sync_send, mut sync_receive) = channel::<()>(100);
    let alice_sync_send = sync_send.clone();
    let bob_sync_send = sync_send;
    let (bob_wallet, bob_storage, alice_wallet, alice_storage, sink_rpc) = init_clients();
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
    test_utils::set_time((EVENT_MATURITY as u64) - 1);

    let electrs =
        Arc::new(EsploraClient::new("http://localhost:30000", bitcoin::Network::Regtest).unwrap());

    let generate_blocks = |nb_blocks: u64| -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let electrs_clone = electrs.clone();
        let sink_clone = sink.clone();
        Box::pin(async move {
            let prev_blockchain_height = electrs_clone.get_blockchain_height().await.unwrap();
            let sink_address = sink_clone
                .get_new_address(None, None)
                .expect("RPC Error")
                .assume_checked();
            sink_clone
                .generate_to_address(nb_blocks, &sink_address)
                .expect("RPC Error");
            // Wait for electrs to have processed the new blocks
            let mut cur_blockchain_height = prev_blockchain_height;
            while cur_blockchain_height < prev_blockchain_height + nb_blocks {
                sleep(std::time::Duration::from_millis(200)).await;
                cur_blockchain_height = electrs_clone.get_blockchain_height().await.unwrap();
            }
        })
    };

    generate_blocks(6).await;

    refresh_wallet(&alice_wallet, 200000000).await;
    refresh_wallet(&bob_wallet, 200000000).await;

    let alice_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&alice_wallet),
            Arc::clone(&alice_wallet),
            Arc::clone(&electrs),
            Arc::clone(&alice_storage),
            alice_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
        )
        .await
        .unwrap(),
    ));

    let alice_manager_loop = Arc::clone(&alice_manager);
    let alice_manager_send = Arc::clone(&alice_manager);

    let bob_manager = Arc::new(Mutex::new(
        Manager::new(
            Arc::clone(&bob_wallet),
            Arc::clone(&bob_wallet),
            Arc::clone(&electrs),
            Arc::clone(&bob_storage),
            bob_oracles,
            Arc::clone(&mock_time),
            Arc::clone(&electrs),
        )
        .await
        .unwrap(),
    ));

    let bob_manager_loop = Arc::clone(&bob_manager);
    let bob_manager_send = Arc::clone(&bob_manager);
    let alice_send_loop = alice_send.clone();
    let bob_send_loop = bob_send.clone();

    let alice_expect_error = Arc::new(AtomicBool::new(false));
    let bob_expect_error = Arc::new(AtomicBool::new(false));

    let alice_expect_error_loop = alice_expect_error.clone();
    let bob_expect_error_loop = bob_expect_error.clone();

    let path_copy = path.clone();
    let alter_sign = move |msg| match msg {
        Message::Sign(mut sign_dlc) => {
            match path_copy {
                TestPath::BadSignCetSignature => {
                    alter_adaptor_sig(&mut sign_dlc.cet_adaptor_signatures)
                }
                TestPath::BadSignRefundSignature => {
                    sign_dlc.refund_signature = alter_refund_sig(&sign_dlc.refund_signature);
                }
                _ => {}
            }
            Some(Message::Sign(sign_dlc))
        }
        _ => Some(msg),
    };

    let msg_callback = |msg: &Message| {
        if let Message::Sign(s) = msg {
            write_message("sign_message", s.clone());
        }
    };

    let alice_handle = receive_loop!(
        alice_receive,
        alice_manager_loop,
        alice_send_loop,
        alice_expect_error_loop,
        alice_sync_send,
        Some,
        msg_callback
    );

    let bob_handle = receive_loop!(
        bob_receive,
        bob_manager_loop,
        bob_send_loop,
        bob_expect_error_loop,
        bob_sync_send,
        alter_sign,
        msg_callback
    );

    let offer_msg = bob_manager_send
        .lock()
        .await
        .send_offer(
            &test_params.contract_input,
            "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
                .parse()
                .unwrap(),
        )
        .await
        .expect("Send offer error");

    write_message("offer_message", offer_msg.clone());
    let temporary_contract_id = offer_msg.temporary_contract_id;
    bob_send
        .send(Some(Message::Offer(offer_msg)))
        .await
        .unwrap();

    assert_contract_state!(bob_manager_send, temporary_contract_id, Offered);

    sync_receive.recv().await.expect("Error synchronizing");

    assert_contract_state!(alice_manager_send, temporary_contract_id, Offered);

    let (contract_id, _, mut accept_msg) = alice_manager_send
        .lock()
        .await
        .accept_contract_offer(&temporary_contract_id)
        .await
        .expect("Error accepting contract offer");

    write_message("accept_message", accept_msg.clone());

    assert_contract_state!(alice_manager_send, contract_id, Accepted);

    match path {
        TestPath::BadAcceptCetSignature | TestPath::BadAcceptRefundSignature => {
            match path {
                TestPath::BadAcceptCetSignature => {
                    alter_adaptor_sig(&mut accept_msg.cet_adaptor_signatures)
                }
                TestPath::BadAcceptRefundSignature => {
                    accept_msg.refund_signature = alter_refund_sig(&accept_msg.refund_signature);
                }
                _ => {}
            };
            bob_expect_error.store(true, Ordering::Relaxed);
            alice_send
                .send(Some(Message::Accept(accept_msg)))
                .await
                .unwrap();
            sync_receive.recv().await.expect("Error synchronizing");
            assert_contract_state!(bob_manager_send, temporary_contract_id, FailedAccept);
        }
        TestPath::BadSignCetSignature | TestPath::BadSignRefundSignature => {
            alice_expect_error.store(true, Ordering::Relaxed);
            alice_send
                .send(Some(Message::Accept(accept_msg)))
                .await
                .unwrap();
            // Bob receives accept message
            sync_receive.recv().await.expect("Error synchronizing");
            // Alice receives sign message
            sync_receive.recv().await.expect("Error synchronizing");
            assert_contract_state!(alice_manager_send, contract_id, FailedSign);
        }
        TestPath::Close | TestPath::Refund => {
            alice_send
                .send(Some(Message::Accept(accept_msg)))
                .await
                .unwrap();
            sync_receive.recv().await.expect("Error synchronizing");

            assert_contract_state!(bob_manager_send, contract_id, Signed);

            // Should not change state and should not error
            periodic_check!(bob_manager_send, contract_id, Signed);

            sync_receive.recv().await.expect("Error synchronizing");

            assert_contract_state!(alice_manager_send, contract_id, Signed);

            alice_wallet.sync().unwrap();
            bob_wallet.sync().unwrap();

            generate_blocks(10).await;

            periodic_check!(alice_manager_send, contract_id, Confirmed);
            periodic_check!(bob_manager_send, contract_id, Confirmed);

            if !manual_close {
                test_utils::set_time((EVENT_MATURITY as u64) + 1);
            }

            // Select the first one to close or refund randomly
            let (first, second) = if thread_rng().next_u32() % 2 == 0 {
                (alice_manager_send, bob_manager_send)
            } else {
                (bob_manager_send, alice_manager_send)
            };

            alice_wallet.sync().unwrap();
            bob_wallet.sync().unwrap();
            match path {
                TestPath::Close => {
                    let case = thread_rng().next_u64() % 3;
                    let blocks: Option<u32> = if case == 2 {
                        Some(10)
                    } else if case == 1 {
                        Some(1)
                    } else {
                        None
                    };

                    if manual_close {
                        periodic_check!(first, contract_id, Confirmed);

                        let attestations = get_attestations(&test_params).await;

                        let f = first.lock().await;
                        let contract = f
                            .close_confirmed_contract(&contract_id, attestations)
                            .await
                            .expect("Error closing contract");

                        alice_wallet.sync().unwrap();
                        bob_wallet.sync().unwrap();

                        if let Contract::PreClosed(contract) = contract {
                            let mut s = second.lock().await;
                            let second_contract =
                                s.get_store().get_contract(&contract_id).unwrap().unwrap();
                            if let Contract::Confirmed(signed) = second_contract {
                                s.on_counterparty_close(
                                    &signed,
                                    contract.signed_cet,
                                    blocks.unwrap_or(0),
                                )
                                .expect("Error registering counterparty close");
                                alice_wallet.sync().unwrap();
                                bob_wallet.sync().unwrap();
                            } else {
                                panic!("Invalid contract state: {:?}", second_contract);
                            }
                        } else {
                            panic!("Invalid contract state {:?}", contract);
                        }
                    } else {
                        alice_wallet.sync().unwrap();
                        bob_wallet.sync().unwrap();
                        periodic_check!(first, contract_id, PreClosed);
                    }

                    // mine blocks for the CET to be confirmed
                    if let Some(b) = blocks {
                        generate_blocks(b as u64).await;
                    }

                    alice_wallet.sync().unwrap();
                    bob_wallet.sync().unwrap();

                    // Randomly check with or without having the CET mined
                    if case == 2 {
                        // cet becomes fully confirmed to blockchain
                        periodic_check!(first, contract_id, Closed);
                        periodic_check!(second, contract_id, Closed);
                    } else {
                        periodic_check!(first, contract_id, PreClosed);
                        periodic_check!(second, contract_id, PreClosed);
                    }
                }
                TestPath::Refund => {
                    alice_wallet.sync().unwrap();
                    bob_wallet.sync().unwrap();
                    periodic_check!(first, contract_id, Confirmed);

                    periodic_check!(second, contract_id, Confirmed);

                    test_utils::set_time(
                        ((EVENT_MATURITY + ddk_manager::manager::REFUND_DELAY) as u64) + 1,
                    );

                    generate_blocks(10).await;

                    alice_wallet.sync().unwrap();
                    bob_wallet.sync().unwrap();

                    periodic_check!(first, contract_id, Refunded);

                    // Randomly check with or without having the Refund mined.
                    if thread_rng().next_u32() % 2 == 0 {
                        generate_blocks(1).await;
                    }

                    alice_wallet.sync().unwrap();
                    bob_wallet.sync().unwrap();

                    periodic_check!(second, contract_id, Refunded);
                }
                _ => unreachable!(),
            }
        }
    }

    alice_send.send(None).await.unwrap();
    bob_send.send(None).await.unwrap();

    alice_handle.await.unwrap();
    bob_handle.await.unwrap();

    create_test_vector().await;
}

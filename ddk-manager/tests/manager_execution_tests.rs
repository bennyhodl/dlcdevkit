#[macro_use]
#[allow(dead_code)]
mod test_utils;

use bitcoin::Amount;
use bitcoincore_rpc::Client;
use ddk::chain::EsploraClient;
use ddk::logger::Logger;
use ddk::oracle::memory::MemoryOracle;
use ddk::storage::memory::MemoryStorage;
use ddk::wallet::DlcDevKitWallet;
use ddk_manager::payout_curve::PayoutFunctionPiece;
use test_utils::*;

use ddk_manager::contract::{
    numerical_descriptor::DifferenceParams, signed_contract::SignedContract, Contract,
};
use ddk_manager::manager::Manager;
use ddk_manager::{
    Blockchain, CachedContractSignerProvider, ContractId, Oracle, SimpleSigner, Storage,
};
use ddk_messages::oracle_msgs::OracleAttestation;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};
use ddk_messages::{CetAdaptorSignatures, Message};
use lightning::ln::wire::Type;
use lightning::util::ser::Writeable;
use secp256k1_zkp::rand::{thread_rng, RngCore};
use secp256k1_zkp::{ecdsa::Signature, EcdsaAdaptorSignature, PublicKey};
use serde_json::from_str;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use test_utils::init_clients;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
#[derive(serde::Serialize, serde::Deserialize)]
struct TestVectorPart<T> {
    message: T,
    #[cfg_attr(
        feature = "use-serde",
        serde(
            serialize_with = "ddk_messages::serde_utils::serialize_hex",
            deserialize_with = "ddk_messages::serde_utils::deserialize_hex_string"
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

async fn numerical_common<F>(
    nb_oracles: usize,
    threshold: usize,
    payout_function_pieces_cb: F,
    difference_params: Option<DifferenceParams>,
    manual_close: bool,
    test_path: TestPath,
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
        test_path,
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
        TestPath::Close,
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

#[derive(Eq, PartialEq, Clone, Debug)]
enum TestPath {
    Close,
    Refund,
    ManualRefund,
    CooperativeClose,
    /// Splice the funded contract one round per entry, then settle whatever the
    /// last round produced.
    Splice(Vec<SpliceRound>),
    BadAcceptCetSignature,
    BadAcceptRefundSignature,
    BadSignCetSignature,
    BadSignRefundSignature,
}

/// Which of the two parties in the test acts.
///
/// Bob offers the contract the test funds and Alice accepts it, so `Bob` is the
/// offering party throughout.
#[derive(Eq, PartialEq, Clone, Copy, Debug)]
enum Party {
    Bob,
    Alice,
}

impl Party {
    fn other(self) -> Party {
        match self {
            Party::Bob => Party::Alice,
            Party::Alice => Party::Bob,
        }
    }
}

/// One round of a splice chain: who offers it, and how it moves the collateral.
#[derive(Eq, PartialEq, Clone, Copy, Debug)]
struct SpliceRound {
    /// The party that offers the replacement contract. It spends the previous
    /// funding output, so it is also the party whose wallet the collateral
    /// moves in from or out to.
    initiator: Party,
    delta: SpliceDelta,
}

impl SpliceRound {
    fn splice_in(initiator: Party, amount: Amount) -> SpliceRound {
        SpliceRound {
            initiator,
            delta: SpliceDelta::In(amount),
        }
    }

    fn splice_out(initiator: Party, amount: Amount) -> SpliceRound {
        SpliceRound {
            initiator,
            delta: SpliceDelta::Out(amount),
        }
    }
}

/// The amount a splice round adds to or removes from the locked collateral.
const SPLICE_AMOUNT: Amount = Amount::from_sat(25_000_000);

/// How far a wallet balance may move beyond the spliced amount and still be
/// explained by transaction fees.
const FEE_SLACK: Amount = Amount::from_sat(100_000);

/// The gap between the maturity of one spliced contract and the next.
///
/// Every round settles on its own event, and the chain only advances the clock
/// once, at the end. Spacing the maturities keeps the assertion that a round
/// closes on its own attestation honest.
const SPLICE_MATURITY_STEP: u32 = 60;

/// How far before [`EVENT_MATURITY`] a splice test starts.
///
/// A splice chain has to stay below every maturity in the chain until the last
/// contract is the one being settled, so it starts further back than the other
/// paths do.
const SPLICE_TIME_HEADROOM: u64 = 3600;

/// The manager the tests drive, with the concrete backends `test_utils` builds.
type TestManager = Manager<
    Arc<DlcDevKitWallet>,
    Arc<CachedContractSignerProvider<Arc<DlcDevKitWallet>, SimpleSigner>>,
    Arc<EsploraClient>,
    Arc<MemoryStorage>,
    Arc<MemoryOracle>,
    Arc<test_utils::MockTime>,
    Arc<EsploraClient>,
    SimpleSigner,
    Arc<Logger>,
>;

/// The two parties, the chain they share, and the wires between them.
///
/// The paths below take this instead of a dozen arguments each. Keeping every
/// path in its own `async fn` also keeps the harness off the stack: a single
/// function holding all of them needs a frame big enough for the largest
/// branch of every one at once.
struct TestContext {
    bob: Arc<Mutex<TestManager>>,
    alice: Arc<Mutex<TestManager>>,
    bob_wallet: Arc<DlcDevKitWallet>,
    alice_wallet: Arc<DlcDevKitWallet>,
    electrs: Arc<EsploraClient>,
    sink: Arc<Client>,
    /// Carries what Bob sends to Alice.
    bob_send: Sender<Option<Message>>,
    /// Carries what Alice sends to Bob.
    alice_send: Sender<Option<Message>>,
    /// Fires once every time either receive loop finishes with a message.
    sync_receive: Receiver<()>,
}

impl TestContext {
    /// Waits for one receive loop to finish handling one message.
    async fn sync(&mut self) {
        self.sync_receive.recv().await.expect("Error synchronizing");
    }

    async fn mine(&self, nb_blocks: u32) {
        generate_blocks(nb_blocks, self.electrs.clone(), self.sink.clone()).await;
    }

    async fn sync_wallets(&self) {
        self.bob_wallet.sync().await.unwrap();
        self.alice_wallet.sync().await.unwrap();
    }

    fn manager(&self, party: Party) -> &Arc<Mutex<TestManager>> {
        match party {
            Party::Bob => &self.bob,
            Party::Alice => &self.alice,
        }
    }

    fn wallet(&self, party: Party) -> &Arc<DlcDevKitWallet> {
        match party {
            Party::Bob => &self.bob_wallet,
            Party::Alice => &self.alice_wallet,
        }
    }

    /// The channel `party` sends on.
    fn sender(&self, party: Party) -> &Sender<Option<Message>> {
        match party {
            Party::Bob => &self.bob_send,
            Party::Alice => &self.alice_send,
        }
    }

    async fn send(&self, party: Party, message: Message) {
        self.sender(party).send(Some(message)).await.unwrap();
    }

    async fn confirmed_balance(&self, party: Party) -> Amount {
        self.wallet(party).get_balance().await.unwrap().confirmed
    }

    async fn contract(&self, party: Party, contract_id: &ContractId) -> Contract {
        self.manager(party)
            .lock()
            .await
            .get_store()
            .get_contract(contract_id)
            .await
            .expect("Could not retrieve contract")
            .expect("Contract does not exist in store")
    }
}

/// The counter party key both managers are configured with.
fn counter_party() -> PublicKey {
    "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
        .parse()
        .unwrap()
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
    numerical_common(
        1,
        1,
        get_hyperbola_payout_curve_pieces,
        None,
        false,
        TestPath::Close,
    )
    .await;
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
async fn enum_single_oracle_manual_refund_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, Some(get_enum_oracles(1, 0).await)).await,
        TestPath::ManualRefund,
        false,
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
async fn single_funded_dlc_test() {
    manager_execution_test(
        get_single_funded_test_params(1, 1).await,
        TestPath::Close,
        false,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Splices
//
// A splice replaces a funded contract with another one, funded by spending the
// first contract's 2-of-2 output. These run the same harness as every other
// path, so a splice is asserted against the same state machine: the tests below
// cover both contract shapes, one and several oracles, thresholds below the
// oracle count, both parties initiating, collateral going both in and out, and
// chains of several splices before settlement.
// ---------------------------------------------------------------------------

/// One splice-in offered by the party that offered the contract.
fn splice_in() -> TestPath {
    TestPath::Splice(vec![SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT)])
}

/// One splice-out offered by the party that offered the contract.
fn splice_out() -> TestPath {
    TestPath::Splice(vec![SpliceRound::splice_out(Party::Bob, SPLICE_AMOUNT)])
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_single_oracle_test() {
    manager_execution_test(get_enum_test_params(1, 1, None).await, splice_in(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_single_oracle_test() {
    manager_execution_test(get_enum_test_params(1, 1, None).await, splice_out(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_single_oracle_manual_test() {
    manager_execution_test(get_enum_test_params(1, 1, None).await, splice_in(), true).await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_single_oracle_manual_test() {
    manager_execution_test(get_enum_test_params(1, 1, None).await, splice_out(), true).await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_3_of_3_test() {
    manager_execution_test(get_enum_test_params(3, 3, None).await, splice_in(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_3_of_3_test() {
    manager_execution_test(get_enum_test_params(3, 3, None).await, splice_out(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_3_of_5_test() {
    manager_execution_test(get_enum_test_params(5, 3, None).await, splice_in(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_3_of_5_test() {
    manager_execution_test(get_enum_test_params(5, 3, None).await, splice_out(), false).await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numerical_single_oracle_test() {
    numerical_common(
        1,
        1,
        get_polynomial_payout_curve_pieces,
        None,
        false,
        splice_in(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numerical_single_oracle_test() {
    numerical_common(
        1,
        1,
        get_polynomial_payout_curve_pieces,
        None,
        false,
        splice_out(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numerical_single_oracle_manual_test() {
    numerical_common(
        1,
        1,
        get_polynomial_payout_curve_pieces,
        None,
        true,
        splice_in(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numerical_3_of_3_test() {
    numerical_common(
        3,
        3,
        get_polynomial_payout_curve_pieces,
        None,
        false,
        splice_in(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numerical_3_of_3_test() {
    numerical_common(
        3,
        3,
        get_polynomial_payout_curve_pieces,
        None,
        false,
        splice_out(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_numerical_with_diff_3_of_5_test() {
    numerical_common(
        5,
        3,
        get_polynomial_payout_curve_pieces,
        Some(get_difference_params()),
        false,
        splice_in(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_numerical_with_diff_3_of_5_test() {
    numerical_common(
        5,
        3,
        get_polynomial_payout_curve_pieces,
        Some(get_difference_params()),
        false,
        splice_out(),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_in_enum_and_numerical_3_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, false, None).await,
        splice_in(),
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_enum_and_numerical_3_of_5_test() {
    manager_execution_test(
        get_enum_and_numerical_test_params(5, 3, false, None).await,
        splice_out(),
        false,
    )
    .await;
}

/// The party that accepted the contract can splice it too. It puts up the whole
/// of the new collateral, because the funding output it spends is credited to
/// whoever offers the replacement.
#[tokio::test]
#[ignore]
async fn splice_in_by_accept_party_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Splice(vec![SpliceRound::splice_in(Party::Alice, SPLICE_AMOUNT)]),
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_out_by_accept_party_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Splice(vec![SpliceRound::splice_out(Party::Alice, SPLICE_AMOUNT)]),
        false,
    )
    .await;
}

/// Collateral in, then out, then in again: each round splices the contract the
/// previous round produced.
#[tokio::test]
#[ignore]
async fn splice_chain_in_out_in_enum_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Splice(vec![
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_out(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
        ]),
        false,
    )
    .await;
}

/// The two parties take turns splicing the same chain.
#[tokio::test]
#[ignore]
async fn splice_chain_alternating_parties_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Splice(vec![
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_out(Party::Alice, SPLICE_AMOUNT),
            SpliceRound::splice_in(Party::Alice, SPLICE_AMOUNT),
        ]),
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_chain_multi_oracle_enum_test() {
    manager_execution_test(
        get_enum_test_params(3, 3, None).await,
        TestPath::Splice(vec![
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_out(Party::Alice, SPLICE_AMOUNT),
        ]),
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn splice_chain_numerical_test() {
    numerical_common(
        1,
        1,
        get_polynomial_payout_curve_pieces,
        None,
        false,
        TestPath::Splice(vec![
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_out(Party::Alice, SPLICE_AMOUNT),
        ]),
    )
    .await;
}

/// A chain settled by hand rather than by the periodic check.
#[tokio::test]
#[ignore]
async fn splice_chain_manual_close_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::Splice(vec![
            SpliceRound::splice_in(Party::Bob, SPLICE_AMOUNT),
            SpliceRound::splice_in(Party::Alice, SPLICE_AMOUNT),
        ]),
        true,
    )
    .await;
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

#[tokio::test]
#[ignore]
async fn cooperative_close_single_oracle_test() {
    manager_execution_test(
        get_enum_test_params(1, 1, None).await,
        TestPath::CooperativeClose,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn cooperative_close_multi_oracle_test() {
    manager_execution_test(
        get_enum_test_params(3, 3, None).await,
        TestPath::CooperativeClose,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn cooperative_close_numerical_test() {
    numerical_polynomial_common(1, 1, None, false).await;
    manager_execution_test(
        get_numerical_test_params(
            &get_same_num_digits_oracle_numeric_infos(1),
            1,
            false,
            get_numerical_contract_descriptor(
                get_same_num_digits_oracle_numeric_infos(1),
                get_polynomial_payout_curve_pieces(NB_DIGITS as usize),
                None,
            ),
            false,
        )
        .await,
        TestPath::CooperativeClose,
        false,
    )
    .await;
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

/// Runs one execution path end to end against its own regtest backends.
///
/// The body runs on a thread of its own so it gets a stack sized for the nested
/// async state machines it drives; see [`test_utils::on_big_stack`].
async fn manager_execution_test(test_params: TestParams, path: TestPath, manual_close: bool) {
    test_utils::on_big_stack(manager_execution_test_inner(
        test_params,
        path,
        manual_close,
    ))
}

async fn manager_execution_test_inner(test_params: TestParams, path: TestPath, manual_close: bool) {
    env_logger::try_init().ok();
    let logger = Arc::new(Logger::disabled("test_manager_execution".to_string()));
    // Held for the whole test: these assertions depend on the chain advancing
    // only when this test mines.
    let env = test_utils::test_env();
    let electrs = test_utils::esplora_client(&env, logger.clone());

    let (alice_send, mut bob_receive) = channel::<Option<Message>>(100);
    let (bob_send, mut alice_receive) = channel::<Option<Message>>(100);
    let (sync_send, sync_receive) = channel::<()>(100);
    let alice_sync_send = sync_send.clone();
    let bob_sync_send = sync_send;
    let amount = Amount::from_btc(2.1).unwrap();
    let (bob_wallet, bob_storage, alice_wallet, alice_storage, sink_rpc) =
        init_clients(&env, logger.clone(), electrs.clone(), amount, amount).await;
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
    // A splice chain has to stay below every maturity in the chain until the
    // last contract is the one being settled, so it starts further back.
    let initial_time = match path {
        TestPath::Splice(_) => (EVENT_MATURITY as u64) - SPLICE_TIME_HEADROOM,
        _ => (EVENT_MATURITY as u64) - 1,
    };

    test_utils::set_time(initial_time);

    test_utils::generate_blocks(6, electrs.clone(), sink.clone()).await;

    refresh_wallet(&alice_wallet, TOTAL_COLLATERAL.to_sat()).await;
    refresh_wallet(&bob_wallet, TOTAL_COLLATERAL.to_sat()).await;

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

    let alice_manager_loop = Arc::clone(&alice_manager);

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

    let bob_manager_loop = Arc::clone(&bob_manager);
    let alice_send_loop = alice_send.clone();
    let bob_send_loop = bob_send.clone();
    let alice_send_shutdown = alice_send.clone();
    let bob_send_shutdown = bob_send.clone();

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

    let mut ctx = TestContext {
        bob: bob_manager,
        alice: alice_manager,
        bob_wallet: Arc::clone(&bob_wallet),
        alice_wallet: Arc::clone(&alice_wallet),
        electrs: Arc::clone(&electrs),
        sink: Arc::clone(&sink),
        bob_send,
        alice_send,
        sync_receive,
    };

    let offer_msg = ctx
        .bob
        .lock()
        .await
        .send_offer(&test_params.contract_input, counter_party())
        .await
        .expect("Send offer error");

    write_message("offer_message", offer_msg.clone());
    let temporary_contract_id = offer_msg.temporary_contract_id;
    ctx.send(Party::Bob, Message::Offer(offer_msg)).await;

    assert_contract_state!(ctx.bob, temporary_contract_id, Offered);

    ctx.sync().await;

    assert_contract_state!(ctx.alice, temporary_contract_id, Offered);

    let (contract_id, _, accept_msg) = ctx
        .alice
        .lock()
        .await
        .accept_contract_offer(&temporary_contract_id)
        .await
        .expect("Error accepting contract offer");

    write_message("accept_message", accept_msg.clone());

    assert_contract_state!(ctx.alice, contract_id, Accepted);

    // Each path lives in its own `async fn` rather than a branch of one big
    // body. That keeps this function's stack frame to the size of a call
    // instead of the largest branch of every path at once.
    match &path {
        TestPath::BadAcceptCetSignature | TestPath::BadAcceptRefundSignature => {
            bad_accept_path(
                &mut ctx,
                &path,
                temporary_contract_id,
                accept_msg,
                &bob_expect_error,
            )
            .await
        }
        TestPath::BadSignCetSignature | TestPath::BadSignRefundSignature => {
            bad_sign_path(&mut ctx, contract_id, accept_msg, &alice_expect_error).await
        }
        TestPath::CooperativeClose => {
            fund_contract(&mut ctx, contract_id, accept_msg).await;
            cooperative_close_path(&mut ctx, contract_id).await
        }
        TestPath::Close => {
            fund_contract(&mut ctx, contract_id, accept_msg).await;
            close_path(&mut ctx, &test_params, contract_id, manual_close).await
        }
        TestPath::Refund | TestPath::ManualRefund => {
            fund_contract(&mut ctx, contract_id, accept_msg).await;
            refund_path(&mut ctx, contract_id, &path, manual_close).await
        }
        TestPath::Splice(rounds) => {
            fund_contract(&mut ctx, contract_id, accept_msg).await;
            splice_path(&mut ctx, &test_params, contract_id, rounds, manual_close).await
        }
    }

    alice_send_shutdown.send(None).await.unwrap();
    bob_send_shutdown.send(None).await.unwrap();

    alice_handle.await.unwrap();
    bob_handle.await.unwrap();

    create_test_vector().await;
}

/// Drives the accepted offer through sign to a confirmed funding transaction.
async fn fund_contract(ctx: &mut TestContext, contract_id: ContractId, accept_msg: AcceptDlc) {
    ctx.send(Party::Alice, Message::Accept(accept_msg)).await;
    ctx.sync().await;

    assert_contract_state!(ctx.bob, contract_id, Signed);

    // Should not change state and should not error
    periodic_check!(ctx.bob, contract_id, Signed);

    ctx.sync().await;

    assert_contract_state!(ctx.alice, contract_id, Signed);

    ctx.sync_wallets().await;

    ctx.mine(10).await;

    periodic_check!(ctx.alice, contract_id, Confirmed);
    periodic_check!(ctx.bob, contract_id, Confirmed);

    ctx.sync_wallets().await;
}

/// Sends an accept message with a corrupted signature and asserts that the
/// offering party rejects it.
async fn bad_accept_path(
    ctx: &mut TestContext,
    path: &TestPath,
    temporary_contract_id: ContractId,
    mut accept_msg: AcceptDlc,
    bob_expect_error: &AtomicBool,
) {
    match path {
        TestPath::BadAcceptCetSignature => {
            alter_adaptor_sig(&mut accept_msg.cet_adaptor_signatures)
        }
        TestPath::BadAcceptRefundSignature => {
            accept_msg.refund_signature = alter_refund_sig(&accept_msg.refund_signature);
        }
        _ => unreachable!(),
    }

    bob_expect_error.store(true, Ordering::Relaxed);
    ctx.send(Party::Alice, Message::Accept(accept_msg)).await;
    ctx.sync().await;
    assert_contract_state!(ctx.bob, temporary_contract_id, FailedAccept);
}

/// Lets the offering party corrupt its own sign message, and asserts that the
/// accepting party rejects it. The corruption happens in Bob's receive loop.
async fn bad_sign_path(
    ctx: &mut TestContext,
    contract_id: ContractId,
    accept_msg: AcceptDlc,
    alice_expect_error: &AtomicBool,
) {
    alice_expect_error.store(true, Ordering::Relaxed);
    ctx.send(Party::Alice, Message::Accept(accept_msg)).await;
    // Bob receives accept message
    ctx.sync().await;
    // Alice receives sign message
    ctx.sync().await;
    assert_contract_state!(ctx.alice, contract_id, FailedSign);
}

/// Settles a confirmed contract with a CET built from oracle attestations.
async fn close_path(
    ctx: &mut TestContext,
    test_params: &TestParams,
    contract_id: ContractId,
    manual_close: bool,
) {
    if !manual_close {
        test_utils::set_time((EVENT_MATURITY as u64) + 1);
    }

    // Select the first one to close randomly
    let first = random_party();
    let second = first.other();

    let case = thread_rng().next_u64() % 3;
    let blocks: Option<u32> = if case == 2 {
        Some(10)
    } else if case == 1 {
        Some(1)
    } else {
        None
    };

    if manual_close {
        periodic_check!(ctx.manager(first), contract_id, Confirmed);

        let attestations = get_attestations(test_params).await;

        let contract = ctx
            .manager(first)
            .lock()
            .await
            .close_confirmed_contract(&contract_id, attestations)
            .await
            .expect("Error closing contract");

        ctx.sync_wallets().await;

        let Contract::PreClosed(contract) = contract else {
            panic!("Invalid contract state {:?}", contract);
        };

        let second_contract = ctx.contract(second, &contract_id).await;
        let Contract::Confirmed(signed) = second_contract else {
            panic!("Invalid contract state: {:?}", second_contract);
        };

        ctx.manager(second)
            .lock()
            .await
            .on_counterparty_close(&signed, contract.signed_cet, blocks.unwrap_or(0))
            .await
            .expect("Error registering counterparty close");

        ctx.sync_wallets().await;
    } else {
        ctx.sync_wallets().await;
        periodic_check!(ctx.manager(first), contract_id, PreClosed);
    }

    // mine blocks for the CET to be confirmed
    if let Some(b) = blocks {
        ctx.mine(b).await;
    }

    ctx.sync_wallets().await;

    // Randomly check with or without having the CET mined
    if case == 2 {
        periodic_check!(ctx.manager(first), contract_id, Closed);
        periodic_check!(ctx.manager(second), contract_id, Closed);
    } else {
        periodic_check!(ctx.manager(first), contract_id, PreClosed);
        periodic_check!(ctx.manager(second), contract_id, PreClosed);
    }
}

/// Runs a confirmed contract past its refund locktime, either letting the
/// periodic check broadcast the refund or broadcasting it by hand.
async fn refund_path(
    ctx: &mut TestContext,
    contract_id: ContractId,
    path: &TestPath,
    manual_close: bool,
) {
    if !manual_close {
        test_utils::set_time((EVENT_MATURITY as u64) + 1);
    }

    let first = random_party();
    let second = first.other();

    ctx.sync_wallets().await;
    periodic_check!(ctx.manager(first), contract_id, Confirmed);
    periodic_check!(ctx.manager(second), contract_id, Confirmed);

    test_utils::set_time(((EVENT_MATURITY + ddk_manager::manager::REFUND_DELAY) as u64) + 1);

    ctx.mine(10).await;
    ctx.sync_wallets().await;

    if path == &TestPath::ManualRefund {
        // Manually broadcast the refund for the first party.
        ctx.manager(first)
            .lock()
            .await
            .check_and_broadcast_refund(&contract_id)
            .await
            .expect("Error manually broadcasting refund");
        assert_contract_state!(ctx.manager(first), contract_id, Refunded);
    } else {
        periodic_check!(ctx.manager(first), contract_id, Refunded);
    }

    // Randomly check with or without having the Refund mined.
    if thread_rng().next_u32() % 2 == 0 {
        ctx.mine(1).await;
    }

    ctx.sync_wallets().await;

    // Second party picks it up via periodic check.
    periodic_check!(ctx.manager(second), contract_id, Refunded);
}

/// Settles a confirmed contract by agreement instead of by attestation.
async fn cooperative_close_path(ctx: &mut TestContext, contract_id: ContractId) {
    // Don't advance time for cooperative close to avoid oracle attestations
    // being available, which would trigger automatic CET closure.

    // First, ensure the funding transaction is confirmed on the blockchain.
    let funding_txid = {
        let alice_contract = ctx.contract(Party::Alice, &contract_id).await;
        let Contract::Confirmed(ref signed_contract) = alice_contract else {
            panic!("Contract should be confirmed");
        };
        signed_contract
            .accepted_contract
            .dlc_transactions
            .fund
            .compute_txid()
    };

    let confirmations = ctx
        .electrs
        .get_transaction_confirmations(&funding_txid)
        .await
        .unwrap();
    assert!(
        confirmations > 0,
        "Funding transaction should be confirmed on blockchain"
    );

    // Alice initiates cooperative close, splitting half to the counter party.
    let counter_payout = Amount::from_sat(ACCEPT_COLLATERAL / 2);

    let (close_msg, _counter_party_pubkey) = ctx
        .alice
        .lock()
        .await
        .cooperative_close_contract(&contract_id, counter_payout)
        .await
        .expect("Error initiating cooperative close");

    // Bob receives and accepts the cooperative close.
    ctx.bob
        .lock()
        .await
        .accept_cooperative_close(&contract_id, &close_msg)
        .await
        .expect("Error accepting cooperative close");

    // Bob broadcast the transaction, so he is the one in PreClosed.
    periodic_check!(ctx.bob, contract_id, PreClosed);

    // Alice does not know about the close yet.
    periodic_check!(ctx.alice, contract_id, Confirmed);

    // Mine a few blocks to partially confirm the close transaction.
    ctx.mine(3).await;

    // Alice now detects the pending close transaction.
    periodic_check!(ctx.alice, contract_id, PreClosed);

    // Bob is still in PreClosed, there are not enough confirmations yet.
    periodic_check!(ctx.bob, contract_id, PreClosed);

    // Mine more blocks to reach full confirmation.
    ctx.mine(5).await;

    periodic_check!(ctx.bob, contract_id, Closed);
    periodic_check!(ctx.alice, contract_id, Closed);

    let bob_contract = ctx.contract(Party::Bob, &contract_id).await;
    let Contract::Closed(ref closed_contract) = bob_contract else {
        panic!("Bob's contract should be in Closed state");
    };
    assert!(
        closed_contract.attestations.is_none(),
        "Cooperative close should not have attestations"
    );
}

/// Replaces a confirmed contract with one holding more or less collateral, by
/// spending its funding output into the funding transaction of a new contract.
///
/// Returns the contract id of the replacement and the parameters it settles
/// on.
///
/// Everything the round claims is asserted here: the replacement reaches
/// Signed while the contract it replaces goes to PreClosed, the replacement
/// confirms while the contract it replaces closes against the very
/// transaction that funded it, the new contract locks exactly the requested
/// collateral, the funding output moves in the requested direction, and the
/// difference comes out of, or goes back into, the splicing party's wallet.
async fn splice_round(
    ctx: &mut TestContext,
    base_params: &TestParams,
    contract_id: ContractId,
    round: usize,
    round_spec: SpliceRound,
    previous_total: Amount,
) -> (ContractId, TestParams) {
    let initiator = round_spec.initiator;
    let acceptor = initiator.other();
    let total_collateral = round_spec.delta.apply(previous_total);
    let maturity = EVENT_MATURITY + (round as u32) * SPLICE_MATURITY_STEP;

    let previous = signed_or_confirmed(ctx.contract(initiator, &contract_id).await);
    let previous_fund = previous
        .accepted_contract
        .dlc_transactions
        .get_fund_output();
    let previous_fund_value = previous_fund.value;
    let previous_funding_txid = previous
        .accepted_contract
        .dlc_transactions
        .fund
        .compute_txid();

    let splice_params = splice_test_params(base_params, round, total_collateral, maturity).await;
    let balance_before = ctx.confirmed_balance(initiator).await;

    let offer_msg = ctx
        .manager(initiator)
        .lock()
        .await
        .send_splice_offer(&splice_params.contract_input, counter_party(), &contract_id)
        .await
        .expect("Send splice offer error");

    let temporary_contract_id = offer_msg.temporary_contract_id;
    ctx.send(initiator, Message::Offer(offer_msg)).await;

    assert_contract_state!(ctx.manager(initiator), temporary_contract_id, Offered);
    ctx.sync().await;
    assert_contract_state!(ctx.manager(acceptor), temporary_contract_id, Offered);

    let (splice_contract_id, _, accept_msg) = ctx
        .manager(acceptor)
        .lock()
        .await
        .accept_contract_offer(&temporary_contract_id)
        .await
        .expect("Error accepting splice offer");

    assert_contract_state!(ctx.manager(acceptor), splice_contract_id, Accepted);

    ctx.send(acceptor, Message::Accept(accept_msg)).await;
    ctx.sync().await;

    // The replacement is signed but not yet mined, so the contract it replaces
    // is spent but not yet closed.
    periodic_check!(ctx.manager(initiator), splice_contract_id, Signed);
    assert_contract_state!(ctx.manager(initiator), contract_id, PreClosed);

    ctx.sync().await;

    periodic_check!(ctx.manager(acceptor), splice_contract_id, Signed);
    assert_contract_state!(ctx.manager(acceptor), contract_id, PreClosed);

    ctx.sync_wallets().await;
    ctx.mine(10).await;
    ctx.sync_wallets().await;

    periodic_check!(ctx.manager(initiator), splice_contract_id, Confirmed);
    periodic_check!(ctx.manager(acceptor), splice_contract_id, Confirmed);
    periodic_check!(ctx.manager(initiator), contract_id, Closed);
    periodic_check!(ctx.manager(acceptor), contract_id, Closed);

    let spliced = signed_or_confirmed(ctx.contract(initiator, &splice_contract_id).await);
    let splice_funding_transaction = spliced.accepted_contract.dlc_transactions.fund.clone();
    let splice_fund_value = spliced
        .accepted_contract
        .dlc_transactions
        .get_fund_output()
        .value;

    assert!(
        splice_funding_transaction
            .input
            .iter()
            .any(|input| input.previous_output.txid == previous_funding_txid),
        "the splice funding transaction must spend the previous funding transaction"
    );

    let dlc_input = spliced
        .accepted_contract
        .offered_contract
        .funding_inputs
        .iter()
        .find_map(|input| input.dlc_input.as_ref())
        .expect("the spliced offer must carry a DLC input");
    assert_eq!(
        dlc_input.contract_id, contract_id,
        "the DLC input must name the contract it replaces"
    );

    assert_eq!(
        spliced.accepted_contract.offered_contract.total_collateral, total_collateral,
        "the spliced contract must lock the requested collateral"
    );

    // The contract that was replaced closes against the transaction that funded
    // its replacement.
    let closed_previous = ctx.contract(initiator, &contract_id).await;
    assert_eq!(
        closed_previous.get_cet_txid().unwrap(),
        splice_funding_transaction.compute_txid(),
        "the replaced contract must close against the splice funding transaction"
    );

    let balance_after = ctx.confirmed_balance(initiator).await;
    match round_spec.delta {
        SpliceDelta::In(amount) => {
            assert!(
                splice_fund_value > previous_fund_value,
                "a splice in must grow the funding output: {previous_fund_value} -> {splice_fund_value}"
            );
            let paid = balance_before.checked_sub(balance_after).unwrap_or_else(|| {
                panic!(
                    "a splice in must not grow the splicing party's wallet: {balance_before} -> {balance_after}"
                )
            });
            assert!(
                paid >= amount && paid <= amount + FEE_SLACK,
                "a splice in of {amount} must come out of the splicing party's wallet, paid {paid}"
            );
        }
        SpliceDelta::Out(amount) => {
            assert!(
                splice_fund_value < previous_fund_value,
                "a splice out must shrink the funding output: {previous_fund_value} -> {splice_fund_value}"
            );
            let received = balance_after.checked_sub(balance_before).unwrap_or_else(|| {
                panic!(
                    "a splice out must not shrink the splicing party's wallet: {balance_before} -> {balance_after}"
                )
            });
            assert!(
                received <= amount && received + FEE_SLACK >= amount,
                "a splice out of {amount} must go back to the splicing party's wallet, received {received}"
            );
        }
    }

    (splice_contract_id, splice_params)
}

/// Splices a confirmed contract once per round, then settles the contract the
/// last round produced and asserts every contract in the chain stayed closed.
async fn splice_path(
    ctx: &mut TestContext,
    test_params: &TestParams,
    contract_id: ContractId,
    rounds: &[SpliceRound],
    manual_close: bool,
) {
    assert!(!rounds.is_empty(), "a splice path needs at least one round");

    let mut replaced = vec![contract_id];
    let mut current = contract_id;
    let mut total = TOTAL_COLLATERAL;
    let mut current_params = None;

    for (index, round_spec) in rounds.iter().enumerate() {
        let round = index + 1;
        let (spliced_id, spliced_params) =
            splice_round(ctx, test_params, current, round, *round_spec, total).await;
        current = spliced_id;
        total = round_spec.delta.apply(total);
        current_params = Some(spliced_params);
        replaced.push(current);
    }

    // Only the last contract in the chain is still open, so advancing past its
    // maturity settles it and nothing else.
    let last_maturity = EVENT_MATURITY + (rounds.len() as u32) * SPLICE_MATURITY_STEP;
    test_utils::set_time(last_maturity as u64 + 1);

    let splice_params = current_params.expect("a splice chain to have run a round");
    settle_spliced_contract(ctx, &splice_params, current, manual_close).await;

    // Every contract the chain replaced stays closed.
    for previous in replaced.iter().take(replaced.len() - 1) {
        assert_contract_state!(ctx.bob, *previous, Closed);
        assert_contract_state!(ctx.alice, *previous, Closed);
    }
}

/// Settles the last contract of a splice chain and asserts its CET spends the
/// splice funding output and pays only the two parties.
async fn settle_spliced_contract(
    ctx: &mut TestContext,
    splice_params: &TestParams,
    contract_id: ContractId,
    manual_close: bool,
) {
    let spliced = signed_or_confirmed(ctx.contract(Party::Bob, &contract_id).await);
    let splice_funding_txid = spliced
        .accepted_contract
        .dlc_transactions
        .fund
        .compute_txid();
    let offer_payout_spk = spliced
        .accepted_contract
        .offered_contract
        .offer_params
        .payout_script_pubkey
        .clone();
    let accept_payout_spk = spliced
        .accepted_contract
        .accept_params
        .payout_script_pubkey
        .clone();

    ctx.sync_wallets().await;

    if manual_close {
        let attestations = get_attestations(splice_params).await;
        let contract = ctx
            .bob
            .lock()
            .await
            .close_confirmed_contract(&contract_id, attestations)
            .await
            .expect("Error closing spliced contract");

        let Contract::PreClosed(contract) = contract else {
            panic!("Invalid contract state {:?}", contract);
        };

        let alice_contract = ctx.contract(Party::Alice, &contract_id).await;
        let Contract::Confirmed(signed) = alice_contract else {
            panic!("Invalid contract state: {:?}", alice_contract);
        };

        ctx.alice
            .lock()
            .await
            .on_counterparty_close(&signed, contract.signed_cet, 0)
            .await
            .expect("Error registering counterparty close");
    } else {
        periodic_check!(ctx.bob, contract_id, PreClosed);
        periodic_check!(ctx.alice, contract_id, PreClosed);
    }

    ctx.mine(10).await;
    ctx.sync_wallets().await;

    periodic_check!(ctx.bob, contract_id, Closed);
    periodic_check!(ctx.alice, contract_id, Closed);

    let Contract::Closed(closed) = ctx.contract(Party::Bob, &contract_id).await else {
        panic!("Spliced contract is not closed");
    };
    let closed_cet = closed.signed_cet.expect("a closed contract to have a CET");

    assert!(
        closed_cet
            .input
            .iter()
            .any(|input| input.previous_output.txid == splice_funding_txid),
        "the CET must spend the splice funding output"
    );
    assert!(
        closed_cet
            .output
            .iter()
            .all(|output| output.script_pubkey == offer_payout_spk
                || output.script_pubkey == accept_payout_spk),
        "the CET must pay only the two parties"
    );

    let status = ctx
        .electrs
        .async_client
        .get_tx_status(&closed_cet.compute_txid())
        .await
        .unwrap();
    assert!(status.confirmed, "the CET must be mined");
}

/// The signed contract inside a `Signed` or `Confirmed` contract.
fn signed_or_confirmed(contract: Contract) -> SignedContract {
    match contract {
        Contract::Signed(signed) | Contract::Confirmed(signed) => signed,
        other => panic!("Contract is neither signed nor confirmed: {:?}", other),
    }
}

fn random_party() -> Party {
    if thread_rng().next_u32() % 2 == 0 {
        Party::Alice
    } else {
        Party::Bob
    }
}

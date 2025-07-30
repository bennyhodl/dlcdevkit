#![allow(dead_code)]

use bitcoin::{
    consensus::{Decodable, Encodable},
    Amount, Network, Transaction,
};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use bitcoincore_rpc_json::AddressType;
use ddk::{chain::EsploraClient, wallet::DlcDevKitWallet};
use ddk::{oracle::memory::MemoryOracle, storage::memory::MemoryStorage};
use ddk_manager::payout_curve::{
    PayoutFunction, PayoutFunctionPiece, PayoutPoint, PolynomialPayoutCurvePiece, RoundingInterval,
    RoundingIntervals,
};
use ddk_manager::Oracle;
use ddk_manager::Time;
use ddk_manager::{
    contract::{
        contract_input::{ContractInput, ContractInputInfo, OracleInput},
        enum_descriptor::EnumDescriptor,
        numerical_descriptor::{DifferenceParams, NumericalDescriptor},
        ContractDescriptor,
    },
    payout_curve::HyperbolaPayoutCurvePiece,
};
use dlc::{EnumerationPayout, Payout};
use dlc_trie::OracleNumericInfo;
use secp256k1_zkp::rand::{seq::SliceRandom, thread_rng, Fill, RngCore};
use std::fmt::Write;
use std::{cell::RefCell, sync::Arc};

pub const NB_DIGITS: u32 = 10;
pub const MIN_SUPPORT_EXP: usize = 1;
pub const MAX_ERROR_EXP: usize = 2;
pub const BASE: u32 = 2;
pub const EVENT_MATURITY: u32 = 1623133104;
pub const EVENT_ID: &str = "Test";
pub const OFFER_COLLATERAL: u64 = 90000000;
pub const ACCEPT_COLLATERAL: u64 = 11000000;
pub const TOTAL_COLLATERAL: Amount = Amount::from_sat(OFFER_COLLATERAL + ACCEPT_COLLATERAL);
pub const MID_POINT: u64 = 5;
pub const ROUNDING_MOD: u64 = 1;

#[macro_export]
macro_rules! receive_loop {
    ($receive:expr, $manager:expr, $send:expr, $expect_err:expr, $sync_send:expr, $rcv_callback: expr, $msg_callback: expr) => {
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            loop {
                match $receive.recv().await {
                    Some(Some(msg)) => match $manager
                        .lock()
                        .await
                        .on_dlc_message(
                            &msg,
                            "0218845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"
                                .parse()
                                .unwrap(),
                        )
                        .await
                    {
                        Ok(opt) => {
                            if $expect_err.load(Ordering::Relaxed) != false {
                                panic!("Expected error not raised");
                            }
                            match opt {
                                Some(msg) => {
                                    let msg_opt = $rcv_callback(msg);
                                    if let Some(msg) = msg_opt {
                                        #[allow(clippy::redundant_closure_call)]
                                        $msg_callback(&msg);
                                        (&$send).send(Some(msg)).await.expect("Error sending");
                                    }
                                }
                                None => {}
                            }
                        }
                        Err(e) => {
                            if $expect_err.load(Ordering::Relaxed) != true {
                                panic!("Unexpected error {}", e);
                            }
                        }
                    },
                    None | Some(None) => return,
                };
                $sync_send.send(()).await.expect("Error syncing");
            }
        })
    };
}

#[macro_export]
macro_rules! write_contract {
    ($contract: ident, $state: ident) => {
        match $contract {
            Contract::$state(s) => {
                let mut buf = Vec::new();
                s.write(&mut buf)
                    .expect("to be able to serialize the contract.");
                std::fs::write(format!("{}", stringify!($state)), buf)
                    .expect("to be able to save the contract to file.");
            }
            _ => {}
        }
    };
}

#[macro_export]
macro_rules! assert_contract_state {
    ($d:expr, $id:expr, $p:ident) => {
        let res = $d
            .lock()
            .await
            .get_store()
            .get_contract(&$id)
            .await
            .expect("Could not retrieve contract");
        if let Some(c) = res {
            if let Contract::$p(_) = c {
            } else {
                panic!("Unexpected contract state {:?}", c);
            }
            if std::env::var("GENERATE_SERIALIZED_CONTRACT").is_ok() {
                write_contract!(c, $p);
            }
        } else {
            panic!("Contract {:02x?} does not exist in store", $id);
        }
    };
}

#[macro_export]
macro_rules! write_channel {
    ($channel: ident, $state: ident) => {
        let suffix = if let Channel::Signed(s) = &$channel {
            format!("{}", s.state)
        } else {
            "".to_string()
        };
        match $channel {
            Channel::$state(s) => {
                let mut buf = Vec::new();
                s.write(&mut buf)
                    .expect("to be able to serialize the channel.");
                std::fs::write(format!("{}Channel{}", stringify!($state), suffix), buf)
                    .expect("to be able to save the channel to file.");
            }
            _ => {}
        }
    };
}

#[macro_export]
macro_rules! assert_channel_state {
    ($d:expr, $id:expr, $p:ident $(, $s: ident)?) => {{
        assert_channel_state_unlocked!($d.lock().await, $id, $p $(, $s)?)
    }};
}

#[allow(unused_macros)]
macro_rules! assert_channel_state_unlocked {
    ($d:expr, $id:expr, $p:ident $(, $s: ident)?) => {{
        let res = $d
            .get_store()
            .get_channel(&$id)
            .expect("Could not retrieve channel");
        if let Some(Channel::$p(c)) = res {
            $(if let ddk_manager::channel::signed_channel::SignedChannelState::$s { .. } = c.state {
            } else {
                panic!("Unexpected signed channel state {:?}", c.state);
            })?
            if std::env::var("GENERATE_SERIALIZED_CHANNEL").is_ok() {
                let channel = Channel::$p(c);
                write_channel!(channel, $p);
            }
        } else if let Some(c) = res {
            panic!("Unexpected channel state {:?}", c);
        } else {
            panic!("Could not find requested channel");
        }
    }};
}

pub fn enum_outcomes() -> Vec<String> {
    vec![
        "a".to_owned(),
        "b".to_owned(),
        "c".to_owned(),
        "d".to_owned(),
    ]
}

pub fn max_value() -> u32 {
    BASE.pow(NB_DIGITS) - 1
}

pub fn max_value_from_digits(nb_digits: usize) -> u32 {
    BASE.pow(nb_digits as u32) - 1
}

pub fn select_active_oracles(nb_oracles: usize, threshold: usize) -> Vec<usize> {
    let nb_active_oracles = if threshold == nb_oracles {
        threshold
    } else {
        (thread_rng().next_u32() % ((nb_oracles - threshold) as u32) + (threshold as u32)) as usize
    };
    let mut oracle_indexes: Vec<usize> = (0..nb_oracles).collect();
    oracle_indexes.shuffle(&mut thread_rng());
    oracle_indexes = oracle_indexes.into_iter().take(nb_active_oracles).collect();
    oracle_indexes.sort_unstable();
    oracle_indexes
}

#[derive(Debug)]
pub struct TestParams {
    pub oracles: Vec<MemoryOracle>,
    pub contract_input: ContractInput,
}

pub fn get_difference_params() -> DifferenceParams {
    DifferenceParams {
        max_error_exp: MAX_ERROR_EXP,
        min_support_exp: MIN_SUPPORT_EXP,
        maximize_coverage: false,
    }
}

pub fn get_enum_contract_descriptor() -> ContractDescriptor {
    let outcome_payouts: Vec<_> = enum_outcomes()
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let payout = if i % 2 == 0 {
                Payout {
                    offer: TOTAL_COLLATERAL,
                    accept: Amount::ZERO,
                }
            } else {
                Payout {
                    offer: Amount::ZERO,
                    accept: TOTAL_COLLATERAL,
                }
            };
            EnumerationPayout {
                outcome: x.to_owned(),
                payout,
            }
        })
        .collect();
    ContractDescriptor::Enum(EnumDescriptor { outcome_payouts })
}

pub async fn generate_blocks(nb_blocks: u32, electrs: Arc<EsploraClient>, sink: Arc<Client>) {
    let prev_blockchain_height = electrs.async_client.get_height().await.unwrap();
    let sink_address = sink
        .get_new_address(None, None)
        .expect("RPC Error")
        .assume_checked();
    sink.generate_to_address(nb_blocks as u64, &sink_address)
        .expect("RPC Error");

    // Use a more stack-friendly polling approach
    let target_height = prev_blockchain_height + nb_blocks;
    loop {
        let current_height = electrs.async_client.get_height().await.unwrap();
        if current_height >= target_height {
            break;
        }

        // Yield control to prevent stack buildup
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

pub async fn get_enum_oracle() -> MemoryOracle {
    let oracle = MemoryOracle::default();

    oracle
        .oracle
        .create_enum_event(EVENT_ID.to_string(), enum_outcomes(), EVENT_MATURITY)
        .await
        .unwrap();

    oracle
}

pub async fn get_enum_oracles(nb_oracles: usize, threshold: usize) -> Vec<MemoryOracle> {
    let mut oracles: Vec<_> = vec![];
    for _ in 0..nb_oracles {
        let oracle = get_enum_oracle().await;
        oracles.push(oracle);
    }

    let active_oracles = select_active_oracles(nb_oracles, threshold);
    let outcomes = enum_outcomes();
    let outcome = outcomes[(thread_rng().next_u32() as usize) % outcomes.len()].clone();
    for index in active_oracles {
        oracles
            .get_mut(index)
            .unwrap()
            .oracle
            .sign_enum_event(EVENT_ID.to_string(), outcome.clone())
            .await
            .unwrap();
    }

    oracles
}

pub async fn get_enum_test_params(
    nb_oracles: usize,
    threshold: usize,
    oracles: Option<Vec<MemoryOracle>>,
) -> TestParams {
    let oracles = match oracles {
        Some(o) => o,
        None => get_enum_oracles(nb_oracles, threshold).await,
    };

    let contract_descriptor = get_enum_contract_descriptor();
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: threshold as u16,
        },
    };

    let contract_input = ContractInput {
        offer_collateral: Amount::from_sat(OFFER_COLLATERAL),
        accept_collateral: Amount::from_sat(ACCEPT_COLLATERAL),
        fee_rate: 2,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub fn get_splice_in_enum_contract_descriptor() -> ContractDescriptor {
    let outcome_payouts: Vec<_> = enum_outcomes()
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let payout = if i % 2 == 0 {
                Payout {
                    offer: TOTAL_COLLATERAL + Amount::from_sat(50_000_000),
                    accept: Amount::ZERO,
                }
            } else {
                Payout {
                    offer: Amount::ZERO,
                    accept: TOTAL_COLLATERAL + Amount::from_sat(50_000_000),
                }
            };
            EnumerationPayout {
                outcome: x.to_owned(),
                payout,
            }
        })
        .collect();
    ContractDescriptor::Enum(EnumDescriptor { outcome_payouts })
}

pub fn get_splice_out_enum_contract_descriptor() -> ContractDescriptor {
    let outcome_payouts: Vec<_> = enum_outcomes()
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let payout = if i % 2 == 0 {
                Payout {
                    offer: TOTAL_COLLATERAL - Amount::from_sat(50_000_000),
                    accept: Amount::ZERO,
                }
            } else {
                Payout {
                    offer: Amount::ZERO,
                    accept: TOTAL_COLLATERAL - Amount::from_sat(50_000_000),
                }
            };
            EnumerationPayout {
                outcome: x.to_owned(),
                payout,
            }
        })
        .collect();
    ContractDescriptor::Enum(EnumDescriptor { outcome_payouts })
}

pub fn get_splice_in_test_params(oracles: Vec<MemoryOracle>) -> TestParams {
    let contract_descriptor = get_splice_in_enum_contract_descriptor();
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: 1,
        },
    };

    let contract_input = ContractInput {
        offer_collateral: TOTAL_COLLATERAL + Amount::from_sat(50_000_000),
        accept_collateral: Amount::ZERO,
        fee_rate: 2,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub fn get_splice_out_test_params(oracles: Vec<MemoryOracle>) -> TestParams {
    let contract_descriptor = get_splice_out_enum_contract_descriptor();
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: 1,
        },
    };

    let contract_input = ContractInput {
        offer_collateral: TOTAL_COLLATERAL - Amount::from_sat(50_000_000),
        accept_collateral: Amount::ZERO,
        fee_rate: 2,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub fn new_oracle_test_params(oracles: Vec<MemoryOracle>) -> TestParams {
    let contract_descriptor = get_splice_in_enum_contract_descriptor();
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: 1,
        },
    };

    let contract_input = ContractInput {
        offer_collateral: TOTAL_COLLATERAL + Amount::from_sat(50_000_000),
        accept_collateral: Amount::ZERO,
        fee_rate: 2,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub async fn get_single_funded_test_params(nb_oracles: usize, threshold: usize) -> TestParams {
    let oracles = get_enum_oracles(nb_oracles, threshold).await;
    let contract_descriptor = get_enum_contract_descriptor();
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: 1,
        },
    };

    let contract_input = ContractInput {
        offer_collateral: TOTAL_COLLATERAL,
        accept_collateral: Amount::ZERO,
        fee_rate: 5,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub fn get_polynomial_payout_curve_pieces(min_nb_digits: usize) -> Vec<PayoutFunctionPiece> {
    vec![
        PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: 0,
                    outcome_payout: Amount::ZERO,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: 3,
                    outcome_payout: Amount::from_sat(OFFER_COLLATERAL),
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: MID_POINT,
                    outcome_payout: TOTAL_COLLATERAL,
                    extra_precision: 0,
                },
            ])
            .unwrap(),
        ),
        PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: MID_POINT,
                    outcome_payout: TOTAL_COLLATERAL,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: max_value_from_digits(min_nb_digits) as u64,
                    outcome_payout: TOTAL_COLLATERAL,
                    extra_precision: 0,
                },
            ])
            .unwrap(),
        ),
    ]
}

pub fn get_hyperbola_payout_curve_pieces(min_nb_digits: usize) -> Vec<PayoutFunctionPiece> {
    vec![PayoutFunctionPiece::HyperbolaPayoutCurvePiece(
        HyperbolaPayoutCurvePiece::new(
            PayoutPoint {
                event_outcome: 0,
                outcome_payout: Amount::ZERO,
                extra_precision: 0,
            },
            PayoutPoint {
                event_outcome: max_value_from_digits(min_nb_digits) as u64,
                outcome_payout: Amount::ZERO,
                extra_precision: 0,
            },
            true,
            50.0,
            50.0,
            5.0,
            -1.0,
            0.0,
            1.0,
        )
        .unwrap(),
    )]
}

pub fn get_numerical_contract_descriptor(
    oracle_numeric_infos: OracleNumericInfo,
    function_pieces: Vec<PayoutFunctionPiece>,
    difference_params: Option<DifferenceParams>,
) -> ContractDescriptor {
    ContractDescriptor::Numerical(NumericalDescriptor {
        payout_function: PayoutFunction::new(function_pieces).unwrap(),
        rounding_intervals: RoundingIntervals {
            intervals: vec![RoundingInterval {
                begin_interval: 0,
                rounding_mod: ROUNDING_MOD,
            }],
        },
        oracle_numeric_infos,
        difference_params,
    })
}

pub async fn get_digit_decomposition_oracle(nb_digits: u16) -> MemoryOracle {
    let oracle = MemoryOracle::default();

    oracle
        .oracle
        .create_numeric_event(
            EVENT_ID.to_string(),
            nb_digits,
            false,
            0,
            "sats/sec".to_owned(),
            EVENT_MATURITY,
        )
        .await
        .unwrap();
    oracle
}

pub async fn get_digit_decomposition_oracles(
    oracle_numeric_infos: &OracleNumericInfo,
    threshold: usize,
    with_diff: bool,
    use_max_value: bool,
) -> Vec<MemoryOracle> {
    let mut oracles = vec![];
    for digit in &oracle_numeric_infos.nb_digits {
        oracles.push(get_digit_decomposition_oracle(*digit as u16).await);
    }

    let outcome_value = if use_max_value {
        max_value_from_digits(oracle_numeric_infos.get_min_nb_digits()) as usize
    } else {
        (thread_rng().next_u32() % max_value()) as usize
    };
    let oracle_indexes = select_active_oracles(oracle_numeric_infos.nb_digits.len(), threshold);

    for (i, index) in oracle_indexes.iter().enumerate() {
        let cur_outcome: usize = if !use_max_value && (i == 0 || !with_diff) {
            outcome_value
        } else if !use_max_value {
            let mut delta = (thread_rng().next_u32() % BASE.pow(MIN_SUPPORT_EXP as u32)) as i32;
            delta = if thread_rng().next_u32() % 2 == 1 {
                -delta
            } else {
                delta
            };

            let tmp_outcome = (outcome_value as i32) + delta;
            if tmp_outcome < 0 {
                0
            } else if tmp_outcome
                > (max_value_from_digits(oracle_numeric_infos.nb_digits[*index]) as i32)
            {
                max_value() as usize
            } else {
                tmp_outcome as usize
            }
        } else {
            let max_value = max_value_from_digits(oracle_numeric_infos.nb_digits[*index]) as usize;
            if max_value == outcome_value {
                outcome_value
            } else {
                outcome_value + 1 + (thread_rng().next_u32() as usize % (max_value - outcome_value))
            }
        };

        for oracle in &oracles {
            let _sign_even_if_it_fails_spent_an_hour_tracking_ci_bug = oracle
                .oracle
                .sign_numeric_event(EVENT_ID.to_string(), cur_outcome as i64)
                .await;
        }
    }

    oracles
}

pub async fn get_numerical_test_params(
    oracle_numeric_infos: &OracleNumericInfo,
    threshold: usize,
    with_diff: bool,
    contract_descriptor: ContractDescriptor,
    use_max_value: bool,
) -> TestParams {
    let oracles =
        get_digit_decomposition_oracles(oracle_numeric_infos, threshold, with_diff, use_max_value)
            .await;
    let contract_info = ContractInputInfo {
        oracles: OracleInput {
            public_keys: oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: threshold as u16,
        },
        contract_descriptor,
    };

    let contract_input = ContractInput {
        offer_collateral: Amount::from_sat(OFFER_COLLATERAL),
        accept_collateral: Amount::from_sat(ACCEPT_COLLATERAL),
        fee_rate: 2,
        contract_infos: vec![contract_info],
    };

    TestParams {
        oracles,
        contract_input,
    }
}

pub async fn get_enum_and_numerical_test_params(
    nb_oracles: usize,
    threshold: usize,
    with_diff: bool,
    difference_params: Option<DifferenceParams>,
) -> TestParams {
    let oracle_numeric_infos = get_same_num_digits_oracle_numeric_infos(nb_oracles);
    let enum_oracles = get_enum_oracles(nb_oracles, threshold).await;
    let enum_contract_descriptor = get_enum_contract_descriptor();
    let enum_contract_info = ContractInputInfo {
        oracles: OracleInput {
            public_keys: enum_oracles.iter().map(|x| x.get_public_key()).collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: threshold as u16,
        },
        contract_descriptor: enum_contract_descriptor,
    };
    let numerical_oracles =
        get_digit_decomposition_oracles(&oracle_numeric_infos, threshold, with_diff, false).await;
    let numerical_contract_descriptor = get_numerical_contract_descriptor(
        get_same_num_digits_oracle_numeric_infos(nb_oracles),
        get_polynomial_payout_curve_pieces(oracle_numeric_infos.get_min_nb_digits()),
        difference_params,
    );
    let numerical_contract_info = ContractInputInfo {
        oracles: OracleInput {
            public_keys: numerical_oracles
                .iter()
                .map(|x| x.get_public_key())
                .collect(),
            event_id: EVENT_ID.to_owned(),
            threshold: threshold as u16,
        },
        contract_descriptor: numerical_contract_descriptor,
    };

    let contract_infos = if thread_rng().next_u32() % 2 == 0 {
        vec![enum_contract_info, numerical_contract_info]
    } else {
        vec![numerical_contract_info, enum_contract_info]
    };

    let contract_input = ContractInput {
        offer_collateral: Amount::from_sat(OFFER_COLLATERAL),
        accept_collateral: Amount::from_sat(ACCEPT_COLLATERAL),
        fee_rate: 2,
        contract_infos,
    };

    TestParams {
        oracles: enum_oracles.into_iter().chain(numerical_oracles).collect(),
        contract_input,
    }
}

pub fn get_same_num_digits_oracle_numeric_infos(nb_oracles: usize) -> OracleNumericInfo {
    OracleNumericInfo {
        nb_digits: std::iter::repeat(NB_DIGITS as usize)
            .take(nb_oracles)
            .collect(),
        base: BASE as usize,
    }
}

pub fn get_variable_oracle_numeric_infos(nb_digits: &[usize]) -> OracleNumericInfo {
    OracleNumericInfo {
        base: BASE as usize,
        nb_digits: nb_digits.to_vec(),
    }
}

pub async fn refresh_wallet(wallet: &DlcDevKitWallet, expected_funds: u64) {
    let mut retry = 0;
    while wallet.get_balance().await.unwrap().confirmed.to_sat() < expected_funds {
        if retry > 30 {
            panic!("Wallet refresh taking too long.")
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        wallet.sync().await.unwrap();
        retry += 1;
    }
}

pub async fn init_clients() -> (
    DlcDevKitWallet,
    Arc<MemoryStorage>,
    DlcDevKitWallet,
    Arc<MemoryStorage>,
    Client,
) {
    let auth = Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let sink_rpc = Client::new(&rpc_base(), auth.clone()).unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let offer_rpc = create_and_fund_wallet().await;
    let accept_rpc = create_and_fund_wallet().await;

    let sink_address = sink_rpc
        .get_new_address(None, Some(AddressType::Bech32))
        .unwrap()
        .assume_checked();

    sink_rpc.generate_to_address(101, &sink_address).unwrap(); //should be 101

    (
        offer_rpc.0,
        offer_rpc.1,
        accept_rpc.0,
        accept_rpc.1,
        sink_rpc,
    )
}

fn rpc_base() -> String {
    let host = std::env::var("BITCOIND_HOST").unwrap_or_else(|_| "localhost".to_owned());
    format!("http://{}:18443", host)
}

pub async fn create_and_fund_wallet() -> (DlcDevKitWallet, Arc<MemoryStorage>) {
    let auth = Auth::UserPass("ddk".to_string(), "ddk".to_string());
    let sink_rpc = Client::new(&rpc_base(), auth.clone()).unwrap();
    let sink_address = sink_rpc
        .get_new_address(None, None)
        .unwrap()
        .assume_checked();
    let mut seed = [0u8; 32];
    seed.try_fill(&mut bitcoin::key::rand::thread_rng())
        .unwrap();
    let memory_storage = Arc::new(MemoryStorage::new());
    let wallet = DlcDevKitWallet::new(
        &seed,
        "http://localhost:30000",
        Network::Regtest,
        memory_storage.clone(),
    )
    .await
    .unwrap();

    let address = wallet.new_external_address().await.unwrap().address;
    sink_rpc
        .send_to_address(
            &address,
            Amount::from_btc(2.1).unwrap(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    sink_rpc.generate_to_address(5, &sink_address).unwrap();
    let mut done = false;
    while !done {
        wallet.sync().await.unwrap();
        let balance = wallet.get_balance().await.unwrap();
        if balance.confirmed > Amount::ZERO {
            done = true;
        }
    }
    (wallet, memory_storage)
}

/// Utility function used to parse hex into a target u8 buffer. Returns
/// the number of bytes converted or an error if it encounters an invalid
/// character or unexpected end of string.
#[allow(clippy::result_unit_err)] // This is just a test util
pub fn from_hex(hex: &str, target: &mut [u8]) -> Result<usize, ()> {
    if hex.len() % 2 == 1 || hex.len() > target.len() * 2 {
        return Err(());
    }

    let mut b = 0;
    let mut idx = 0;
    for c in hex.bytes() {
        b <<= 4;
        match c {
            b'A'..=b'F' => b |= c - b'A' + 10,
            b'a'..=b'f' => b |= c - b'a' + 10,
            b'0'..=b'9' => b |= c - b'0',
            _ => return Err(()),
        }
        if (idx & 1) == 1 {
            target[idx / 2] = b;
            b = 0;
        }
        idx += 1;
    }
    Ok(idx / 2)
}

/// Transforms an hex string to a Vec<u8>.
/// Panics if the string is not valid hex.
pub fn str_to_hex(hex_str: &str) -> Vec<u8> {
    let mut hex = vec![0; hex_str.len() / 2];
    from_hex(hex_str, &mut hex).unwrap();
    hex
}

/// Serialize a transaction to an lower hex string.
pub fn tx_to_string(tx: &Transaction) -> String {
    let mut writer = Vec::new();
    tx.consensus_encode(&mut writer).unwrap();
    let mut serialized = String::new();
    for x in writer {
        write!(&mut serialized, "{:02x}", x).unwrap();
    }
    serialized
}

/// Deserialize an hex string to a bitcoin transaction.
/// Panics if given invalid hex or data.
pub fn tx_from_string(tx_str: &str) -> Transaction {
    let tx_hex = str_to_hex(tx_str);
    Transaction::consensus_decode(&mut tx_hex.as_slice()).unwrap()
}

thread_local! {
  static MOCK_TIME: RefCell<u64> = RefCell::new(0);
}

pub struct MockTime {}

impl Time for MockTime {
    fn unix_time_now(&self) -> u64 {
        MOCK_TIME.with(|f| *f.borrow())
    }
}

pub fn set_time(time: u64) {
    MOCK_TIME.with(|f| {
        *f.borrow_mut() = time;
    });
}

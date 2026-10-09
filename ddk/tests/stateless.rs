//! Lifecycle tests for the stateless contract API.
//!
//! Every test completes (or rejects) a DLC using only wire messages, explicit
//! party data, and PSBTs — no storage backend, contract manager, or
//! blockchain client is constructed anywhere in this file.

use bdk_wallet::template::Bip49;
use bdk_wallet::{KeychainKind, SignOptions, Wallet};
use bitcoin::absolute::LockTime;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::hashes::Hash;
use bitcoin::psbt::Psbt;
use bitcoin::transaction::Version;
use bitcoin::{Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use ddk::contract::{
    accept_offer, chain_hash_from_network, create_dlc_splice_input, create_dlc_transactions,
    create_funding_psbt, create_offer, create_signed_dlc_transactions, finalize_sign,
    finalize_sign_spliced, funding_input, sign_accept, sign_accept_spliced, sign_cet, sign_refund,
    signing, validate_offer, AcceptOfferParams, ContractError, CreateOfferParams, DescriptorInput,
    DlcInputSigningKey, InputDerivation, Party, PartyParams, DLC_INPUT_MAX_WITNESS_LEN,
};
use ddk_dlc::secp256k1_zkp::{All, Keypair, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey};
use ddk_messages::contract_msgs::{
    ContractDescriptor, ContractInfo, ContractInfoInner, ContractOutcome, DisjointContractInfo,
    EnumeratedContractDescriptor, NumericOutcomeContractDescriptor, SingleContractInfo,
};
use ddk_messages::oracle_msgs::{
    tagged_announcement_msg, tagged_attestation_msg, DigitDecompositionEventDescriptor,
    EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
    OracleInfo, SingleOracleInfo,
};
use ddk_messages::{
    AcceptDlc, FundingInput, OfferDlc, OverrideOutcome, PayoutScriptOverride,
    PayoutScriptOverrides, SignDlc, WitnessElement,
};
use std::str::FromStr;

const NETWORK: Network = Network::Regtest;
const MIN_TIMEOUT: u32 = 100;
const MAX_TIMEOUT: u32 = 500;
/// The accepting party's clock, one second before the test oracle event
/// matures.
const NOW_UNIX: u64 = 749;
const TOTAL_COLLATERAL: Amount = Amount::from_sat(100_000);

/// One side of a contract: a DLC funding key plus a wallet key controlling a
/// single funding UTXO. The payout and change go to `payout_spk`.
struct PartySetup {
    funding_secret_key: SecretKey,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    funding_input: FundingInput,
    payout_spk: ScriptBuf,
}

impl PartySetup {
    fn new(
        secp: &Secp256k1<All>,
        seed_byte: u8,
        network: Network,
        utxo_value: Amount,
        input_serial_id: u64,
    ) -> Self {
        Self::with_wrapping(secp, seed_byte, network, utxo_value, input_serial_id, false)
    }

    /// Like [`PartySetup::new`], but the funding UTXO is P2SH-P2WPKH when
    /// `wrapped` is set: the P2WPKH script becomes the redeem script.
    fn with_wrapping(
        secp: &Secp256k1<All>,
        seed_byte: u8,
        network: Network,
        utxo_value: Amount,
        input_serial_id: u64,
        wrapped: bool,
    ) -> Self {
        let funding_secret_key = SecretKey::from_slice(&[seed_byte; 32]).unwrap();
        let xpriv = Xpriv::new_master(network, &[seed_byte.wrapping_add(100); 64]).unwrap();
        let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
        let derivation_path =
            DerivationPath::from_str(&format!("84h/{coin_type}h/0h/0/0")).unwrap();
        let witness_script = p2wpkh_script(secp, &xpriv, &derivation_path);
        let (script_pubkey, redeem_script) = if wrapped {
            (
                ScriptBuf::new_p2sh(&witness_script.script_hash()),
                witness_script.clone(),
            )
        } else {
            (witness_script.clone(), ScriptBuf::new())
        };
        let previous_transaction = previous_transaction(utxo_value, script_pubkey);
        let funding_input = funding_input(
            &previous_transaction,
            0,
            Some(input_serial_id),
            u32::MAX,
            108,
            redeem_script,
        )
        .unwrap();
        Self {
            funding_secret_key,
            xpriv,
            derivation_path,
            funding_input,
            payout_spk: witness_script,
        }
    }

    /// A party whose funding UTXO, payout, and change all belong to a BDK
    /// BIP49 (`sh(wpkh())`) wallet. The wallet is returned so a test can let
    /// BDK sign the funding PSBT itself.
    fn bip49(
        _secp: &Secp256k1<All>,
        seed_byte: u8,
        network: Network,
        utxo_value: Amount,
        input_serial_id: u64,
    ) -> (Wallet, Self) {
        let funding_secret_key = SecretKey::from_slice(&[seed_byte; 32]).unwrap();
        let xpriv = Xpriv::new_master(network, &[seed_byte.wrapping_add(100); 64]).unwrap();
        let mut wallet = Wallet::create(
            Bip49(xpriv, KeychainKind::External),
            Bip49(xpriv, KeychainKind::Internal),
        )
        .network(network)
        .create_wallet_no_persist()
        .unwrap();

        let address = wallet.reveal_next_address(KeychainKind::External);
        let script_pubkey = address.script_pubkey();
        assert!(script_pubkey.is_p2sh(), "BIP49 addresses are P2SH");
        let redeem_script = wallet
            .public_descriptor(KeychainKind::External)
            .at_derivation_index(address.index)
            .unwrap()
            .explicit_script()
            .unwrap();
        assert!(redeem_script.is_p2wpkh(), "BIP49 redeem script is P2WPKH");
        assert_eq!(
            ScriptBuf::new_p2sh(&redeem_script.script_hash()),
            script_pubkey
        );

        // Hand the wallet its funding UTXO so BDK can find the key on signing.
        // BDK rejects a coinbase as an unconfirmed transaction, so the
        // previous transaction spends a dummy non-null outpoint.
        let mut previous_transaction = previous_transaction(utxo_value, script_pubkey);
        previous_transaction.input[0].previous_output = OutPoint {
            txid: bitcoin::Txid::from_slice(&[seed_byte; 32]).unwrap(),
            vout: 0,
        };
        wallet.apply_unconfirmed_txs([(previous_transaction.clone(), 0u64)]);
        let utxo = wallet
            .list_unspent()
            .next()
            .expect("wallet tracks the funding UTXO");
        assert_eq!(utxo.txout.value, utxo_value);

        let funding_input = funding_input(
            &previous_transaction,
            utxo.outpoint.vout,
            Some(input_serial_id),
            u32::MAX,
            108,
            redeem_script,
        )
        .unwrap();
        let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
        let derivation_path =
            DerivationPath::from_str(&format!("49h/{coin_type}h/0h/0/{}", address.index)).unwrap();
        let payout_spk = wallet
            .reveal_next_address(KeychainKind::External)
            .script_pubkey();
        let party = Self {
            funding_secret_key,
            xpriv,
            derivation_path,
            funding_input,
            payout_spk,
        };
        (wallet, party)
    }

    fn funding_pubkey(&self, secp: &Secp256k1<All>) -> PublicKey {
        self.funding_secret_key.public_key(secp)
    }

    fn payout_script(&self, _secp: &Secp256k1<All>) -> ScriptBuf {
        self.payout_spk.clone()
    }

    fn party_params(
        &self,
        secp: &Secp256k1<All>,
        funding_inputs: Vec<FundingInput>,
    ) -> PartyParams {
        PartyParams {
            funding_pubkey: self.funding_pubkey(secp),
            funding_inputs,
            payout_spk: self.payout_script(secp),
            payout_serial_id: None,
            change_spk: self.payout_script(secp),
            change_serial_id: None,
        }
    }

    fn derivations(&self) -> Vec<InputDerivation> {
        vec![InputDerivation {
            input_serial_id: self.funding_input.input_serial_id,
            derivation_path: self.derivation_path.clone(),
        }]
    }
}

fn p2wpkh_script(secp: &Secp256k1<All>, xpriv: &Xpriv, path: &DerivationPath) -> ScriptBuf {
    let public_key = xpriv
        .derive_priv(secp, path)
        .unwrap()
        .to_priv()
        .public_key(secp);
    ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap())
}

fn previous_transaction(value: Amount, script_pubkey: ScriptBuf) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value,
            script_pubkey,
        }],
    }
}

fn oracle_announcement(
    event_descriptor: EventDescriptor,
    nonce_count: usize,
) -> OracleAnnouncement {
    let secp = Secp256k1::new();
    let oracle_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[88; 32]).unwrap());
    let oracle_nonces = (0..nonce_count)
        .map(|index| {
            let nonce_key = Keypair::from_secret_key(
                &secp,
                &SecretKey::from_slice(&[90 + index as u8; 32]).unwrap(),
            );
            XOnlyPublicKey::from_keypair(&nonce_key).0
        })
        .collect();
    let oracle_event = OracleEvent {
        oracle_nonces,
        event_maturity_epoch: 750,
        event_descriptor,
        event_id: "stateless-test".to_string(),
    };
    OracleAnnouncement {
        announcement_signature: secp
            .sign_schnorr(&tagged_announcement_msg(&oracle_event), &oracle_key),
        oracle_public_key: XOnlyPublicKey::from_keypair(&oracle_key).0,
        oracle_event,
    }
}

/// An enum contract that pays the whole collateral to the offering party on
/// "up" and to the accepting party on "down".
fn enum_contract_info(total_collateral: Amount) -> ContractInfo {
    enum_contract_info_paying(total_collateral, total_collateral)
}

/// An enum contract paying `up_offer_payout` of `total_collateral` to the
/// offering party on "up", and nothing on "down".
fn enum_contract_info_paying(total_collateral: Amount, up_offer_payout: Amount) -> ContractInfo {
    let announcement = oracle_announcement(
        EventDescriptor::EnumEvent(EnumEventDescriptor {
            outcomes: vec!["up".to_string(), "down".to_string()],
        }),
        1,
    );
    ContractInfo::SingleContractInfo(SingleContractInfo {
        total_collateral,
        contract_info: ContractInfoInner {
            contract_descriptor: ContractDescriptor::EnumeratedContractDescriptor(
                EnumeratedContractDescriptor {
                    payouts: vec![
                        ContractOutcome {
                            outcome: "up".to_string(),
                            offer_payout: up_offer_payout,
                        },
                        ContractOutcome {
                            outcome: "down".to_string(),
                            offer_payout: Amount::ZERO,
                        },
                    ],
                },
            ),
            oracle_info: OracleInfo::Single(SingleOracleInfo {
                oracle_announcement: announcement,
            }),
        },
    })
}

fn numerical_contract_info(offer_collateral: Amount, accept_collateral: Amount) -> ContractInfo {
    numerical_contract_info_rounded(offer_collateral, accept_collateral, 1)
}

/// A numerical contract whose payouts round to `rounding_mod` sats: a coarser
/// rounding gives fewer distinct payouts, so fewer CETs.
fn numerical_contract_info_rounded(
    offer_collateral: Amount,
    accept_collateral: Amount,
    rounding_mod: u64,
) -> ContractInfo {
    let nb_digits = 10u16;
    let max_value = (1u64 << nb_digits) - 1;
    let payout_function = ddk_payouts::generate_payout_curve(
        0,
        900,
        offer_collateral,
        accept_collateral,
        5,
        max_value,
    )
    .unwrap();
    let numerical = ddk_manager::contract::numerical_descriptor::NumericalDescriptor {
        payout_function,
        rounding_intervals: ddk_manager::payout_curve::RoundingIntervals {
            intervals: vec![ddk_manager::payout_curve::RoundingInterval {
                begin_interval: 0,
                rounding_mod,
            }],
        },
        difference_params: None,
        oracle_numeric_infos: ddk_trie::OracleNumericInfo {
            base: 2,
            nb_digits: vec![nb_digits as usize],
        },
    };
    let announcement = oracle_announcement(
        EventDescriptor::DigitDecompositionEvent(DigitDecompositionEventDescriptor {
            base: 2,
            is_signed: false,
            unit: "sats".to_string(),
            precision: 0,
            nb_digits,
        }),
        nb_digits as usize,
    );
    ContractInfo::SingleContractInfo(SingleContractInfo {
        total_collateral: offer_collateral + accept_collateral,
        contract_info: ContractInfoInner {
            contract_descriptor: ContractDescriptor::NumericOutcomeContractDescriptor(
                NumericOutcomeContractDescriptor::from(&numerical),
            ),
            oracle_info: OracleInfo::Single(SingleOracleInfo {
                oracle_announcement: announcement,
            }),
        },
    })
}

fn offer_params(
    secp: &Secp256k1<All>,
    offerer: &PartySetup,
    contract_info: ContractInfo,
    offer_collateral: Amount,
    network: Network,
    funding_inputs: Vec<FundingInput>,
) -> CreateOfferParams {
    CreateOfferParams {
        chain_hash: chain_hash_from_network(network),
        temporary_contract_id: None,
        contract_info,
        offer_collateral,
        party: offerer.party_params(secp, funding_inputs),
        fund_output_serial_id: None,
        fee_rate_per_vb: 2,
        cet_locktime: Some(750),
        refund_locktime: 1_000,
        contract_flags: 0,
    }
}

/// Builds an enum contract offer/accept pair with one funding input per party.
fn enum_contract(
    secp: &Secp256k1<All>,
    network: Network,
) -> (PartySetup, PartySetup, OfferDlc, AcceptDlc) {
    let offerer = PartySetup::new(secp, 1, network, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(secp, 2, network, Amount::from_sat(150_000), 2);
    enum_contract_between(secp, network, offerer, accepter)
}

/// Builds an enum contract offer/accept pair between two prepared parties.
fn enum_contract_between(
    secp: &Secp256k1<All>,
    network: Network,
    offerer: PartySetup,
    accepter: PartySetup,
) -> (PartySetup, PartySetup, OfferDlc, AcceptDlc) {
    let offer = create_offer(offer_params(
        secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        network,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(secp, vec![accepter.funding_input.clone()]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    (offerer, accepter, offer, accept_result.accept)
}

/// Runs sign_accept and finalize_sign with xpriv-signed PSBTs and checks the
/// completed funding transaction.
fn complete_with_xpriv(
    secp: &Secp256k1<All>,
    offerer: &PartySetup,
    accepter: &PartySetup,
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> Transaction {
    fund_with_xpriv(secp, offerer, accepter, offer, accept).1
}

/// Like [`complete_with_xpriv`], but also returns the sign message, which the
/// settlement functions need.
fn fund_with_xpriv(
    _secp: &Secp256k1<All>,
    offerer: &PartySetup,
    accepter: &PartySetup,
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> (SignDlc, Transaction) {
    let mut offer_psbt = create_funding_psbt(offer, accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        offer,
        accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    let sign_result = sign_accept(offer, accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let mut accept_psbt = create_funding_psbt(offer, accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        offer,
        accept,
        &mut accept_psbt,
        &accepter.xpriv,
        &accepter.derivations(),
    )
    .unwrap();
    let funding_transaction =
        finalize_sign(offer, accept, &sign_result.sign, &accept_psbt).unwrap();

    assert_funding_transaction_complete(&funding_transaction, offer, accept);
    (sign_result.sign, funding_transaction)
}

/// Checks that every funding input carries a witness whose public key matches
/// the previous output it spends.
fn assert_funding_transaction_complete(
    funding_transaction: &Transaction,
    offer: &OfferDlc,
    accept: &AcceptDlc,
) {
    let transactions = create_dlc_transactions(offer, accept).unwrap();
    assert_eq!(
        funding_transaction.compute_txid(),
        transactions.fund.compute_txid()
    );
    let prevouts: Vec<(OutPoint, TxOut)> = offer
        .funding_inputs
        .iter()
        .chain(&accept.funding_inputs)
        .map(|input| {
            let transaction: Transaction = bitcoin::consensus::deserialize(&input.prev_tx).unwrap();
            (
                OutPoint {
                    txid: transaction.compute_txid(),
                    vout: input.prev_tx_vout,
                },
                transaction.output[input.prev_tx_vout as usize].clone(),
            )
        })
        .collect();
    for tx_input in &funding_transaction.input {
        let (_, prevout) = prevouts
            .iter()
            .find(|(outpoint, _)| *outpoint == tx_input.previous_output)
            .expect("funding transaction spends an unknown outpoint");
        assert_eq!(tx_input.witness.len(), 2, "expected P2WPKH witness");
        let public_key = bitcoin::PublicKey::from_slice(&tx_input.witness[1]).unwrap();
        let witness_script = ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap());
        if prevout.script_pubkey.is_p2sh() {
            assert_eq!(
                prevout.script_pubkey,
                ScriptBuf::new_p2sh(&witness_script.script_hash()),
                "witness key does not control the wrapped output"
            );
            let push = bitcoin::script::PushBytesBuf::try_from(witness_script.to_bytes()).unwrap();
            assert_eq!(
                tx_input.script_sig,
                ScriptBuf::builder().push_slice(push).into_script(),
                "wrapped input must push its redeem script"
            );
        } else {
            assert_eq!(
                prevout.script_pubkey, witness_script,
                "witness key does not control the spent output"
            );
            assert!(
                tx_input.script_sig.is_empty(),
                "native SegWit input has no script sig"
            );
        }
    }
}

#[test]
fn enum_lifecycle_with_xpriv_signing() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    complete_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
}

#[test]
fn numerical_lifecycle_with_xpriv_signing() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 11, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 12, NETWORK, Amount::from_sat(150_000), 2);
    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        numerical_contract_info(Amount::from_sat(50_000), Amount::from_sat(50_000)),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    complete_with_xpriv(&secp, &offerer, &accepter, &offer, &accept_result.accept);
}

#[test]
fn an_offer_with_a_cet_locktime_after_maturity_is_not_created() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 1, NETWORK, Amount::from_sat(150_000), 1);
    let mut params = offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offerer.funding_input.clone()],
    );
    let maturity = params.contract_info.get_closest_maturity_date();

    params.cet_locktime = Some(maturity + 1);
    create_offer(params.clone()).expect_err("a CET locktime after maturity");

    params.cet_locktime = Some(maturity);
    create_offer(params).expect("a CET locktime at maturity");
}

#[test]
fn mainnet_bip32_paths_complete_the_lifecycle() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, Network::Bitcoin);
    assert_eq!(offer.chain_hash, chain_hash_from_network(Network::Bitcoin));
    complete_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
}

#[test]
fn descriptor_signing_completes_the_lifecycle() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);

    // The offer party signs with a private wildcard descriptor.
    let descriptor = format!("wpkh({}/84h/1h/0h/0/*)", offerer.xpriv);
    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut offer_psbt,
        &descriptor,
        &[DescriptorInput {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter.xpriv,
        &accepter.derivations(),
    )
    .unwrap();
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).unwrap();
    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
}

#[test]
fn watch_only_descriptor_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, _, offer, accept) = enum_contract(&secp, NETWORK);
    let account = offerer
        .xpriv
        .derive_priv(&secp, &DerivationPath::from_str("84h/1h/0h").unwrap())
        .unwrap();
    let xpub = bitcoin::bip32::Xpub::from_priv(&secp, &account);
    let descriptor = format!("wpkh({xpub}/0/*)");
    let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
    let result = signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut psbt,
        &descriptor,
        &[DescriptorInput {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    );
    assert!(matches!(result, Err(ContractError::Descriptor(_))));
}

#[test]
fn wrong_descriptor_index_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, _, offer, accept) = enum_contract(&secp, NETWORK);
    let descriptor = format!("wpkh({}/84h/1h/0h/0/*)", offerer.xpriv);
    let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
    let result = signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut psbt,
        &descriptor,
        &[DescriptorInput {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_index: 7,
        }],
    );
    assert!(matches!(result, Err(ContractError::Descriptor(_))));
}

#[test]
fn incorrect_derivation_path_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, _, offer, accept) = enum_contract(&secp, NETWORK);
    let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
    let result = signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut psbt,
        &offerer.xpriv,
        &[InputDerivation {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_path: DerivationPath::from_str("84h/1h/0h/0/9").unwrap(),
        }],
    );
    assert!(matches!(result, Err(ContractError::InvalidFundingInput(_))));
}

#[tokio::test]
async fn wallet_interface_signs_the_funding_psbt() {
    let secp = Secp256k1::new();
    let offerer_wallet = TestWallet::new(1);
    let accepter_wallet = TestWallet::new(2);

    let offerer = PartySetup::new(&secp, 21, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 22, NETWORK, Amount::from_sat(150_000), 2);
    // Fund each party from its wallet's first address instead of the xpriv key.
    let offer_input = funding_input(
        &previous_transaction(Amount::from_sat(150_000), offerer_wallet.script_pubkey()),
        0,
        Some(1),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap();
    let accept_input = funding_input(
        &previous_transaction(Amount::from_sat(150_000), accepter_wallet.script_pubkey()),
        0,
        Some(2),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap();

    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offer_input],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![accept_input]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    let accept = accept_result.accept;

    let mut offer_psbt = accept_result.funding_psbt.clone();
    signing::sign_funding_psbt_with_wallet(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer_wallet,
        Party::Offer,
    )
    .await
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_wallet(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter_wallet,
        Party::Accept,
    )
    .await
    .unwrap();
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).unwrap();
    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
}

#[test]
fn externally_finalized_psbt_completes_the_lifecycle() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);

    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    // The accept party hands the PSBT to an "external wallet": the PSBT is
    // serialized, signed and finalized with plain rust-bitcoin, and returned.
    let psbt = create_funding_psbt(&offer, &accept).unwrap();
    let serialized = psbt.serialize();
    let externally_signed = external_wallet_sign(
        serialized,
        &accepter.xpriv,
        &accepter.derivation_path,
        &secp,
    );
    let returned = Psbt::deserialize(&externally_signed).unwrap();

    let funding_transaction = finalize_sign(&offer, &accept, &sign_result.sign, &returned).unwrap();
    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
}

/// Simulates an external wallet: signs and finalizes only the inputs it owns
/// using nothing but rust-bitcoin.
fn external_wallet_sign(
    serialized_psbt: Vec<u8>,
    xpriv: &Xpriv,
    path: &DerivationPath,
    secp: &Secp256k1<All>,
) -> Vec<u8> {
    let mut psbt = Psbt::deserialize(&serialized_psbt).unwrap();
    let private_key = xpriv.derive_priv(secp, path).unwrap().to_priv();
    let public_key = private_key.public_key(secp);
    let owned_script = ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap());
    let fingerprint = xpriv.fingerprint(secp);
    for index in 0..psbt.inputs.len() {
        let owns_input = psbt.inputs[index]
            .witness_utxo
            .as_ref()
            .map(|utxo| utxo.script_pubkey == owned_script)
            .unwrap_or(false);
        if !owns_input {
            continue;
        }
        psbt.inputs[index]
            .bip32_derivation
            .insert(public_key.inner, (fingerprint, path.clone()));
    }
    psbt.sign(xpriv, secp).unwrap();
    for index in 0..psbt.inputs.len() {
        let Some((public_key, signature)) = psbt.inputs[index]
            .partial_sigs
            .iter()
            .map(|(pk, sig)| (*pk, *sig))
            .next()
        else {
            continue;
        };
        psbt.inputs[index].final_script_witness = Some(Witness::from_slice(&[
            signature.to_vec(),
            public_key.to_bytes(),
        ]));
        psbt.inputs[index].partial_sigs.clear();
    }
    psbt.serialize()
}

#[test]
fn single_funded_contract_with_no_accept_inputs() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 31, NETWORK, Amount::from_sat(250_000), 1);
    let accepter = PartySetup::new(&secp, 32, NETWORK, Amount::from_sat(150_000), 2);
    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        TOTAL_COLLATERAL,
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    let accept = accept_result.accept;
    assert_eq!(accept.accept_collateral, Amount::ZERO);
    assert!(accept.funding_inputs.is_empty());

    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    // No accept-side inputs to sign: the unsigned PSBT is sufficient.
    let unsigned_psbt = create_funding_psbt(&offer, &accept).unwrap();
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &unsigned_psbt).unwrap();
    assert_eq!(funding_transaction.input.len(), 1);
    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
}

#[test]
fn shuffled_serial_ids_map_witnesses_to_the_right_inputs() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 41, NETWORK, Amount::from_sat(75_000), 900);
    let accepter = PartySetup::new(&secp, 42, NETWORK, Amount::from_sat(150_000), 37);
    // Second offer input with a serial id sorting before the accept input.
    let second_path = DerivationPath::from_str("84h/1h/0h/0/1").unwrap();
    let second_input = funding_input(
        &previous_transaction(
            Amount::from_sat(75_000),
            p2wpkh_script(&secp, &offerer.xpriv, &second_path),
        ),
        0,
        Some(5),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap();

    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offerer.funding_input.clone(), second_input],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    let accept = accept_result.accept;

    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &[
            InputDerivation {
                input_serial_id: 900,
                derivation_path: offerer.derivation_path.clone(),
            },
            InputDerivation {
                input_serial_id: 5,
                derivation_path: second_path,
            },
        ],
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter.xpriv,
        &accepter.derivations(),
    )
    .unwrap();
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).unwrap();
    assert_eq!(funding_transaction.input.len(), 3);
    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
}

#[test]
fn mutated_psbt_transactions_are_rejected() {
    let secp = Secp256k1::new();
    let (offerer, _, offer, accept) = enum_contract(&secp, NETWORK);

    let sign_with = |psbt: &Psbt| sign_accept(&offer, &accept, &offerer.funding_secret_key, psbt);
    let signed_psbt = {
        let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
        signing::sign_funding_psbt_with_xpriv(
            &offer,
            &accept,
            &mut psbt,
            &offerer.xpriv,
            &offerer.derivations(),
        )
        .unwrap();
        psbt
    };

    // Modified output value.
    let mut mutated = signed_psbt.clone();
    mutated.unsigned_tx.output[0].value += Amount::from_sat(1);
    assert!(matches!(
        sign_with(&mutated),
        Err(ContractError::PsbtMismatch(_))
    ));

    // Modified locktime.
    let mut mutated = signed_psbt.clone();
    mutated.unsigned_tx.lock_time = LockTime::from_consensus(777);
    assert!(matches!(
        sign_with(&mutated),
        Err(ContractError::PsbtMismatch(_))
    ));

    // Modified sequence.
    let mut mutated = signed_psbt.clone();
    mutated.unsigned_tx.input[0].sequence = Sequence::ZERO;
    assert!(matches!(
        sign_with(&mutated),
        Err(ContractError::PsbtMismatch(_))
    ));

    // Modified outpoint.
    let mut mutated = signed_psbt.clone();
    mutated.unsigned_tx.input[0].previous_output.vout = 9;
    assert!(matches!(
        sign_with(&mutated),
        Err(ContractError::PsbtMismatch(_))
    ));

    // The signing sources reject mutated PSBTs too.
    let mut mutated = create_funding_psbt(&offer, &accept).unwrap();
    mutated.unsigned_tx.output[0].value += Amount::from_sat(1);
    assert!(matches!(
        signing::sign_funding_psbt_with_xpriv(
            &offer,
            &accept,
            &mut mutated,
            &offerer.xpriv,
            &offerer.derivations(),
        ),
        Err(ContractError::PsbtMismatch(_))
    ));
}

#[test]
fn missing_finalized_witness_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);

    let unsigned_psbt = create_funding_psbt(&offer, &accept).unwrap();
    assert!(matches!(
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &unsigned_psbt),
        Err(ContractError::MissingFinalizedInput { .. })
    ));

    // finalize_sign requires the accept-side witness even when the offer side
    // already signed.
    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();
    let _ = accepter;
    assert!(matches!(
        finalize_sign(&offer, &accept, &sign_result.sign, &unsigned_psbt),
        Err(ContractError::MissingFinalizedInput { .. })
    ));
}

#[test]
fn invalid_counterparty_adaptor_signatures_are_rejected() {
    let secp = Secp256k1::new();
    let (offerer, _, offer, mut accept) = enum_contract(&secp, NETWORK);
    accept
        .cet_adaptor_signatures
        .ecdsa_adaptor_signatures
        .reverse();

    let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    assert!(matches!(
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &psbt),
        Err(ContractError::InvalidAccept(_))
    ));
}

#[test]
fn incorrect_contract_id_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);

    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut offer_psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();
    let mut sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();
    sign_result.sign.contract_id[0] ^= 0xff;

    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut accept_psbt,
        &accepter.xpriv,
        &accepter.derivations(),
    )
    .unwrap();
    assert!(matches!(
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt),
        Err(ContractError::InvalidSign(_))
    ));
}

#[test]
fn accept_result_psbt_matches_create_funding_psbt() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 51, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 52, NETWORK, Amount::from_sat(150_000), 2);
    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept_result = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    let rebuilt = create_funding_psbt(&offer, &accept_result.accept).unwrap();
    assert_eq!(accept_result.funding_psbt.serialize(), rebuilt.serialize());
    assert_eq!(
        accept_result.transactions.fund.compute_txid(),
        create_dlc_transactions(&offer, &accept_result.accept)
            .unwrap()
            .fund
            .compute_txid()
    );
}

/// Attests `outcomes` with the same oracle and nonce keys
/// [`oracle_announcement`] publishes, producing an attestation the contract
/// accepts as genuine.
fn oracle_attestation(outcomes: Vec<String>) -> OracleAttestation {
    let secp = Secp256k1::new();
    let oracle_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[88; 32]).unwrap());
    let signatures = outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            let nonce = SecretKey::from_slice(&[90 + index as u8; 32]).unwrap();
            ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
                &secp,
                &tagged_attestation_msg(outcome),
                &oracle_key,
                &nonce.secret_bytes(),
            )
        })
        .collect();
    OracleAttestation {
        event_id: "stateless-test".to_string(),
        oracle_public_key: XOnlyPublicKey::from_keypair(&oracle_key).0,
        signatures,
        outcomes,
    }
}

/// Decomposes `value` into the fixed-width binary digit strings a digit
/// decomposition oracle attests.
fn digit_outcomes(value: u64, nb_digits: usize) -> Vec<String> {
    (0..nb_digits)
        .rev()
        .map(|position| ((value >> position) & 1).to_string())
        .collect()
}

/// Checks that a settlement transaction spends the funding output with a
/// complete 2-of-2 witness.
fn assert_spends_funding_output(
    settlement: &Transaction,
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_transaction: &Transaction,
) {
    let transactions = create_dlc_transactions(offer, accept).unwrap();
    assert_eq!(settlement.input.len(), 1);
    assert_eq!(
        settlement.input[0].previous_output,
        OutPoint {
            txid: funding_transaction.compute_txid(),
            vout: transactions.get_fund_output_index() as u32,
        }
    );
    let witness = &settlement.input[0].witness;
    assert_eq!(witness.len(), 4, "expected a 2-of-2 witness");
    assert!(witness[0].is_empty(), "multisig witness must start empty");
    assert_eq!(
        witness[3],
        transactions.funding_witness_script.to_bytes(),
        "witness script is not the funding script"
    );
}

#[test]
fn either_party_can_settle_with_a_cet() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
    let attestations = vec![(0, oracle_attestation(vec!["up".to_string()]))];

    // "up" pays the whole contract to the offering party.
    let cet = sign_cet(
        &offer,
        &accept,
        &sign,
        &offerer.funding_secret_key,
        &attestations,
    )
    .unwrap();
    assert_spends_funding_output(&cet, &offer, &accept, &funding_transaction);
    assert_eq!(cet.output.len(), 1);
    assert_eq!(cet.output[0].script_pubkey, offer.payout_spk);

    // The accepting party settles the same outcome independently, and lands on
    // the same transaction: the witness differs only by which half each party
    // produced, and the txid does not commit to it.
    let counterpart = sign_cet(
        &offer,
        &accept,
        &sign,
        &accepter.funding_secret_key,
        &attestations,
    )
    .unwrap();
    assert_spends_funding_output(&counterpart, &offer, &accept, &funding_transaction);
    assert_eq!(cet.compute_txid(), counterpart.compute_txid());
}

#[test]
fn the_attested_outcome_selects_the_cet() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    // "down" pays the whole contract to the accepting party instead.
    let cet = sign_cet(
        &offer,
        &accept,
        &sign,
        &accepter.funding_secret_key,
        &[(0, oracle_attestation(vec!["down".to_string()]))],
    )
    .unwrap();
    assert_eq!(cet.output.len(), 1);
    assert_eq!(cet.output[0].script_pubkey, accept.payout_spk);
}

#[test]
fn numerical_contracts_settle_with_a_cet() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 61, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 62, NETWORK, Amount::from_sat(150_000), 2);
    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        numerical_contract_info(Amount::from_sat(50_000), Amount::from_sat(50_000)),
        Amount::from_sat(50_000),
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept = accept_offer(
        &offer,
        AcceptOfferParams {
            party: accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .unwrap()
    .accept;
    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    // A digit decomposition attestation may be consumed as a prefix, so the
    // number of oracle signatures used is decided by the CET that matched.
    let cet = sign_cet(
        &offer,
        &accept,
        &sign,
        &offerer.funding_secret_key,
        &[(0, oracle_attestation(digit_outcomes(500, 10)))],
    )
    .unwrap();
    assert_spends_funding_output(&cet, &offer, &accept, &funding_transaction);
}

#[test]
fn either_party_can_settle_with_the_refund() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
    let transactions = create_dlc_transactions(&offer, &accept).unwrap();

    for funding_secret_key in [offerer.funding_secret_key, accepter.funding_secret_key] {
        let refund = sign_refund(&offer, &accept, &sign, &funding_secret_key).unwrap();
        assert_spends_funding_output(&refund, &offer, &accept, &funding_transaction);
        assert_eq!(
            refund.compute_txid(),
            transactions.refund.compute_txid(),
            "the refund must be the one rebuilt from the messages"
        );
        // Each party gets its own collateral back.
        assert_eq!(refund.output.len(), 2);
        assert!(refund
            .output
            .iter()
            .any(|output| output.script_pubkey == offer.payout_spk));
        assert!(refund
            .output
            .iter()
            .any(|output| output.script_pubkey == accept.payout_spk));
    }
}

#[test]
fn settling_with_a_foreign_key_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
    let stranger = SecretKey::from_slice(&[77; 32]).unwrap();

    assert!(matches!(
        sign_cet(
            &offer,
            &accept,
            &sign,
            &stranger,
            &[(0, oracle_attestation(vec!["up".to_string()]))],
        ),
        Err(ContractError::Key(_))
    ));
    assert!(matches!(
        sign_refund(&offer, &accept, &sign, &stranger),
        Err(ContractError::Key(_))
    ));
}

#[test]
fn an_unknown_outcome_has_no_cet() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    assert!(matches!(
        sign_cet(
            &offer,
            &accept,
            &sign,
            &offerer.funding_secret_key,
            &[(0, oracle_attestation(vec!["sideways".to_string()]))],
        ),
        Err(ContractError::NoMatchingOutcome)
    ));
}

#[test]
fn a_forged_attestation_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    // A well-formed attestation for a real outcome, signed by an oracle the
    // contract does not use.
    let impostor = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[13; 32]).unwrap());
    let nonce = SecretKey::from_slice(&[14; 32]).unwrap();
    let forged = OracleAttestation {
        event_id: "stateless-test".to_string(),
        oracle_public_key: XOnlyPublicKey::from_keypair(&impostor).0,
        signatures: vec![ddk_dlc::secp_utils::schnorrsig_sign_with_nonce(
            &secp,
            &tagged_attestation_msg("up"),
            &impostor,
            &nonce.secret_bytes(),
        )],
        outcomes: vec!["up".to_string()],
    };

    assert!(matches!(
        sign_cet(
            &offer,
            &accept,
            &sign,
            &offerer.funding_secret_key,
            &[(0, forged)],
        ),
        Err(ContractError::InvalidAttestation(_))
    ));
}

#[test]
fn an_out_of_range_oracle_index_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    // The contract uses a single oracle, so index 4 does not exist. The enum
    // outcome lookup does not range check the index it is handed, so this is
    // caught when the attestation is checked against the announcement it claims
    // to come from — without which the CET would be signed with the wrong
    // adaptor signature.
    let error = sign_cet(
        &offer,
        &accept,
        &sign,
        &offerer.funding_secret_key,
        &[(4, oracle_attestation(vec!["up".to_string()]))],
    )
    .unwrap_err();
    assert!(
        matches!(error, ContractError::InvalidAttestation(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn a_duplicated_oracle_attestation_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    // The same genuine attestation twice. Without the index check its
    // signature would be summed twice into the adaptor secret and the CET would
    // carry an invalid counterparty signature.
    let attestation = oracle_attestation(vec!["up".to_string()]);
    let error = sign_cet(
        &offer,
        &accept,
        &sign,
        &offerer.funding_secret_key,
        &[(0, attestation.clone()), (0, attestation)],
    )
    .unwrap_err();
    assert!(
        matches!(error, ContractError::InvalidAttestation(_)),
        "unexpected error: {error}"
    );
}

#[test]
fn settling_with_a_mismatched_sign_message_is_rejected() {
    let secp = Secp256k1::new();
    let (offerer, accepter, offer, accept) = enum_contract(&secp, NETWORK);
    let (mut sign, _) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
    sign.contract_id[0] ^= 0xff;

    assert!(matches!(
        sign_cet(
            &offer,
            &accept,
            &sign,
            &offerer.funding_secret_key,
            &[(0, oracle_attestation(vec!["up".to_string()]))],
        ),
        Err(ContractError::InvalidSign(_))
    ));
    assert!(matches!(
        sign_refund(&offer, &accept, &sign, &offerer.funding_secret_key),
        Err(ContractError::InvalidSign(_))
    ));
}

/// A minimal wallet implementing [`ddk_manager::Wallet`] over an in-memory
/// BDK wallet. Only PSBT signing is exercised by the stateless API.
struct TestWallet {
    wallet: std::sync::Mutex<bdk_wallet::Wallet>,
    script_pubkey: ScriptBuf,
}

impl TestWallet {
    fn new(seed_byte: u8) -> Self {
        let xpriv = Xpriv::new_master(NETWORK, &[seed_byte; 64]).unwrap();
        let descriptor = format!("wpkh({xpriv}/84h/1h/0h/0/*)");
        let mut wallet = bdk_wallet::Wallet::create_single(descriptor)
            .network(NETWORK)
            .create_wallet_no_persist()
            .unwrap();
        let address = wallet.reveal_next_address(bdk_wallet::KeychainKind::External);
        Self {
            wallet: std::sync::Mutex::new(wallet),
            script_pubkey: address.address.script_pubkey(),
        }
    }

    fn script_pubkey(&self) -> ScriptBuf {
        self.script_pubkey.clone()
    }
}

#[async_trait::async_trait]
impl ddk_manager::Wallet for TestWallet {
    async fn get_new_address(&self) -> Result<bitcoin::Address, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn get_new_change_address(&self) -> Result<bitcoin::Address, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn get_utxos_for_amount(
        &self,
        _amount: Amount,
        _fee_rate: u64,
        _lock_utxos: bool,
    ) -> Result<Vec<ddk_manager::Utxo>, ddk_manager::error::Error> {
        unimplemented!("not needed for PSBT signing")
    }
    async fn sign_psbt_input(
        &self,
        psbt: &mut Psbt,
        input_index: usize,
    ) -> Result<(), ddk_manager::error::Error> {
        let wallet = self.wallet.lock().unwrap();
        let mut signed = psbt.clone();
        let options = bdk_wallet::SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        wallet
            .sign(&mut signed, options)
            .map_err(|e| ddk_manager::error::Error::WalletError(Box::new(e)))?;
        psbt.inputs[input_index] = signed.inputs[input_index].clone();
        Ok(())
    }
    fn import_address(&self, _address: &bitcoin::Address) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }
    fn unreserve_utxos(&self, _outpoints: &[OutPoint]) -> Result<(), ddk_manager::error::Error> {
        Ok(())
    }
}

/// The offer-side signing state for a splice, produced by [`prepare_splice`]
/// and finalized either by [`complete_splice`] or directly in a negative test.
struct PreparedSplice {
    offer_b: OfferDlc,
    accept_b: AcceptDlc,
    sign: SignDlc,
    accept_psbt: Psbt,
    unsigned_fund_b: Transaction,
    splice_serial: u64,
    splice_input: FundingInput,
    prior_accept_key: SecretKey,
    prior_offer_key: SecretKey,
    /// The offering party's funding key and PSBT for contract B, so a test can
    /// re-run the signing step with a different splice key.
    offerer_b_funding_secret_key: SecretKey,
    offer_psbt: Psbt,
    fund_outpoint_a: OutPoint,
    fund_value_a: Amount,
}

/// Builds and fully signs contract A, then builds a single-funded contract B
/// whose offer spends A's funding output as a splice input, and produces the
/// offering party's sign message (with its half of the splice signature).
fn prepare_splice(splice_in: bool) -> PreparedSplice {
    let secp = Secp256k1::new();

    // Contract A: an ordinary dual-funded enum contract, fully signed.
    let (offerer_a, accepter_a, offer_a, accept_a) = enum_contract(&secp, NETWORK);
    let (sign_a, funding_tx_a) =
        fund_with_xpriv(&secp, &offerer_a, &accepter_a, &offer_a, &accept_a);
    let transactions_a = create_dlc_transactions(&offer_a, &accept_a).unwrap();
    let fund_value_a = transactions_a.get_fund_output().value;
    let fund_outpoint_a = OutPoint {
        txid: funding_tx_a.compute_txid(),
        vout: transactions_a.get_fund_output_index() as u32,
    };

    // The splice input spends A's 2-of-2 funding output.
    let splice_serial = 900;
    let splice_input = create_dlc_splice_input(
        &offer_a,
        &accept_a,
        &sign_a,
        Party::Offer,
        Some(splice_serial),
        DLC_INPUT_MAX_WITNESS_LEN,
    )
    .unwrap();

    // Contract B is single-funded by the offering party with fresh funding keys.
    let offerer_b = PartySetup::new(&secp, 5, NETWORK, Amount::from_sat(200_000), 10);
    let accepter_b = PartySetup::new(&secp, 6, NETWORK, Amount::from_sat(200_000), 11);
    let splice_amount = Amount::from_sat(40_000);
    let (offer_collateral_b, offer_funding_inputs) = if splice_in {
        (
            fund_value_a + splice_amount,
            vec![splice_input.clone(), offerer_b.funding_input.clone()],
        )
    } else {
        (fund_value_a - splice_amount, vec![splice_input.clone()])
    };

    let offer_b = create_offer(offer_params(
        &secp,
        &offerer_b,
        enum_contract_info(offer_collateral_b),
        offer_collateral_b,
        NETWORK,
        offer_funding_inputs,
    ))
    .unwrap();
    let accept_b = accept_offer(
        &offer_b,
        AcceptOfferParams {
            party: accepter_b.party_params(&secp, vec![]),
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter_b.funding_secret_key,
    )
    .unwrap()
    .accept;

    // The offering party signs its wallet input (splice-in only) and its half of
    // the prior 2-of-2 with the previous contract's funding key.
    let mut offer_psbt = create_funding_psbt(&offer_b, &accept_b).unwrap();
    if splice_in {
        signing::sign_funding_psbt_with_xpriv(
            &offer_b,
            &accept_b,
            &mut offer_psbt,
            &offerer_b.xpriv,
            &offerer_b.derivations(),
        )
        .unwrap();
    }
    let offer_splice_key = DlcInputSigningKey {
        input_serial_id: splice_serial,
        prior_funding_secret_key: offerer_a.funding_secret_key,
    };
    let sign = sign_accept_spliced(
        &offer_b,
        &accept_b,
        &offerer_b.funding_secret_key,
        &offer_psbt,
        std::slice::from_ref(&offer_splice_key),
    )
    .unwrap()
    .sign;

    let accept_psbt = create_funding_psbt(&offer_b, &accept_b).unwrap();
    let unsigned_fund_b = create_dlc_transactions(&offer_b, &accept_b).unwrap().fund;

    PreparedSplice {
        offer_b,
        accept_b,
        sign,
        accept_psbt,
        unsigned_fund_b,
        splice_serial,
        splice_input,
        prior_accept_key: accepter_a.funding_secret_key,
        prior_offer_key: offerer_a.funding_secret_key,
        offerer_b_funding_secret_key: offerer_b.funding_secret_key,
        offer_psbt,
        fund_outpoint_a,
        fund_value_a,
    }
}

/// Runs [`prepare_splice`] and completes the funding transaction on the
/// accepting side, contributing its half of the splice signature.
fn complete_splice(splice_in: bool) -> (Transaction, PreparedSplice) {
    let prepared = prepare_splice(splice_in);
    let accept_splice_key = DlcInputSigningKey {
        input_serial_id: prepared.splice_serial,
        prior_funding_secret_key: prepared.prior_accept_key,
    };
    let funding_tx_b = finalize_sign_spliced(
        &prepared.offer_b,
        &prepared.accept_b,
        &prepared.sign,
        &prepared.accept_psbt,
        std::slice::from_ref(&accept_splice_key),
    )
    .unwrap();
    (funding_tx_b, prepared)
}

/// Asserts the completed funding transaction spends contract A's funding output
/// with a valid, combined 2-of-2 witness.
fn assert_splice_input_signed(funding_tx_b: &Transaction, prepared: &PreparedSplice) {
    let input_index = funding_tx_b
        .input
        .iter()
        .position(|tx_in| tx_in.previous_output == prepared.fund_outpoint_a)
        .expect("splice funding transaction must spend contract A's funding output");
    assert_eq!(
        funding_tx_b.compute_txid(),
        prepared.unsigned_fund_b.compute_txid()
    );

    let witness: Vec<Vec<u8>> = funding_tx_b.input[input_index]
        .witness
        .iter()
        .map(|element| element.to_vec())
        .collect();
    assert_eq!(witness.len(), 4, "expected a 2-of-2 witness");
    assert!(witness[0].is_empty(), "multisig witness must start empty");

    let dlc_input_info: ddk_dlc::dlc_input::DlcInputInfo = (&prepared.splice_input).into();
    let expected_script = ddk_dlc::make_funding_redeemscript(
        &dlc_input_info.local_fund_pubkey,
        &dlc_input_info.remote_fund_pubkey,
    );
    assert_eq!(witness[3], expected_script.to_bytes());

    // Each half signature must verify against one of the prior 2-of-2 keys.
    let secp = Secp256k1::new();
    for pubkey in [
        dlc_input_info.local_fund_pubkey,
        dlc_input_info.remote_fund_pubkey,
    ] {
        assert!(
            [&witness[1], &witness[2]].into_iter().any(|signature| {
                ddk_dlc::dlc_input::verify_dlc_funding_input_signature(
                    &secp,
                    &prepared.unsigned_fund_b,
                    input_index,
                    &dlc_input_info,
                    signature.clone(),
                    &pubkey,
                )
                .is_ok()
            }),
            "no signature verifies against a prior funding key"
        );
    }
}

#[test]
fn splice_in_completes_the_lifecycle() {
    let (funding_tx_b, prepared) = complete_splice(true);
    assert_splice_input_signed(&funding_tx_b, &prepared);
    let fund_value_b = create_dlc_transactions(&prepared.offer_b, &prepared.accept_b)
        .unwrap()
        .get_fund_output()
        .value;
    assert!(
        fund_value_b > prepared.fund_value_a,
        "splice-in must increase the funded amount"
    );
}

#[test]
fn splice_out_completes_the_lifecycle() {
    let (funding_tx_b, prepared) = complete_splice(false);
    assert_splice_input_signed(&funding_tx_b, &prepared);
    let fund_value_b = create_dlc_transactions(&prepared.offer_b, &prepared.accept_b)
        .unwrap()
        .get_fund_output()
        .value;
    assert!(
        fund_value_b < prepared.fund_value_a,
        "splice-out must decrease the funded amount"
    );
}

#[test]
fn finalize_sign_spliced_rejects_a_wrong_prior_key() {
    let prepared = prepare_splice(true);
    // A key that does not control the prior 2-of-2 output.
    let wrong_key = DlcInputSigningKey {
        input_serial_id: prepared.splice_serial,
        prior_funding_secret_key: SecretKey::from_slice(&[9; 32]).unwrap(),
    };
    assert!(matches!(
        finalize_sign_spliced(
            &prepared.offer_b,
            &prepared.accept_b,
            &prepared.sign,
            &prepared.accept_psbt,
            std::slice::from_ref(&wrong_key),
        ),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

/// The two keys in a [`DlcInput`] are ordered by who offers the splice. A
/// caller that gets that order wrong — by naming the wrong side of the previous
/// contract — holds a key that controls the 2-of-2 but is the wrong half of it,
/// and the signing side must say so rather than produce a signature nobody can
/// use.
#[test]
fn sign_accept_spliced_rejects_the_counterparty_prior_key() {
    let prepared = prepare_splice(true);
    // A real key for the prior 2-of-2, but the other half of it.
    let counterparty_key = DlcInputSigningKey {
        input_serial_id: prepared.splice_serial,
        prior_funding_secret_key: prepared.prior_accept_key,
    };
    assert!(matches!(
        sign_accept_spliced(
            &prepared.offer_b,
            &prepared.accept_b,
            &prepared.offerer_b_funding_secret_key,
            &prepared.offer_psbt,
            std::slice::from_ref(&counterparty_key),
        ),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

/// The same inversion on the accepting side: a key that controls the prior
/// 2-of-2 but matches `local_fund_pubkey` rather than `remote_fund_pubkey`.
#[test]
fn finalize_sign_spliced_rejects_the_counterparty_prior_key() {
    let prepared = prepare_splice(true);
    let counterparty_key = DlcInputSigningKey {
        input_serial_id: prepared.splice_serial,
        prior_funding_secret_key: prepared.prior_offer_key,
    };
    assert!(matches!(
        finalize_sign_spliced(
            &prepared.offer_b,
            &prepared.accept_b,
            &prepared.sign,
            &prepared.accept_psbt,
            std::slice::from_ref(&counterparty_key),
        ),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

#[test]
fn finalize_sign_spliced_rejects_a_tampered_offer_half() {
    let prepared = prepare_splice(true);
    let secp = Secp256k1::new();
    let dlc_input_info: ddk_dlc::dlc_input::DlcInputInfo = (&prepared.splice_input).into();
    let input_index = prepared
        .unsigned_fund_b
        .input
        .iter()
        .position(|tx_in| tx_in.previous_output == prepared.fund_outpoint_a)
        .unwrap();
    // A signature by the remote (accept) key verifies against remote_fund_pubkey,
    // not local_fund_pubkey, so the offer-half verification must reject it.
    let wrong_half = ddk_dlc::dlc_input::create_dlc_funding_input_signature(
        &secp,
        &prepared.unsigned_fund_b,
        input_index,
        &dlc_input_info,
        &prepared.prior_accept_key,
    )
    .unwrap();
    let mut tampered = prepared.sign.clone();
    let position = prepared
        .offer_b
        .funding_inputs
        .iter()
        .position(|input| input.dlc_input.is_some())
        .unwrap();
    tampered.funding_signatures.funding_signatures[position].witness_elements =
        vec![WitnessElement {
            witness: wrong_half,
        }];
    let accept_splice_key = DlcInputSigningKey {
        input_serial_id: prepared.splice_serial,
        prior_funding_secret_key: prepared.prior_accept_key,
    };
    assert!(matches!(
        finalize_sign_spliced(
            &prepared.offer_b,
            &prepared.accept_b,
            &tampered,
            &prepared.accept_psbt,
            std::slice::from_ref(&accept_splice_key),
        ),
        Err(ContractError::InvalidSign(_))
    ));
}

#[test]
fn p2sh_p2wpkh_funding_inputs_carry_the_redeem_script_push() {
    // A wrapped SegWit input needs its script signature to push the redeem
    // script, next to the witness. Until this was fixed, building the funding
    // transaction for such an input panicked.
    let secp = Secp256k1::new();
    let offerer = PartySetup::with_wrapping(&secp, 1, NETWORK, Amount::from_sat(150_000), 1, true);
    let accepter = PartySetup::new(&secp, 2, NETWORK, Amount::from_sat(150_000), 2);
    let (offerer, accepter, offer, accept) =
        enum_contract_between(&secp, NETWORK, offerer, accepter);

    let funding_transaction = complete_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    let offerer_prev_tx: Transaction =
        bitcoin::consensus::deserialize(&offerer.funding_input.prev_tx).unwrap();
    let offerer_outpoint = OutPoint {
        txid: offerer_prev_tx.compute_txid(),
        vout: offerer.funding_input.prev_tx_vout,
    };
    let redeem_script = offerer.funding_input.redeem_script.clone();
    let expected_script_sig = ScriptBuf::builder()
        .push_slice(bitcoin::script::PushBytesBuf::try_from(redeem_script.to_bytes()).unwrap())
        .into_script();

    let mut saw_wrapped_input = false;
    for input in &funding_transaction.input {
        if input.previous_output == offerer_outpoint {
            assert_eq!(input.script_sig, expected_script_sig);
            assert_eq!(
                input.witness.len(),
                2,
                "P2WPKH witness is signature + pubkey"
            );
            saw_wrapped_input = true;
        } else {
            assert!(
                input.script_sig.is_empty(),
                "native SegWit inputs stay empty"
            );
        }
    }
    assert!(
        saw_wrapped_input,
        "the wrapped input is spent by the funding transaction"
    );
}

/// Checks that every input the wallet owns is finalized with a redeem script
/// push and a two-element P2WPKH witness, and that the counterparty's inputs
/// are untouched.
fn assert_bdk_signed_own_inputs(psbt: &Psbt, wallet: &Wallet) {
    for input in &psbt.inputs {
        let script_pubkey = &input.witness_utxo.as_ref().unwrap().script_pubkey;
        if wallet.is_mine(script_pubkey.clone()) {
            let witness = input
                .final_script_witness
                .as_ref()
                .expect("BDK finalizes its own input");
            assert_eq!(witness.len(), 2, "P2WPKH witness is signature + pubkey");
            // BDK clears the redeem_script field on finalize, so derive the
            // expected push from the wallet descriptor.
            let (keychain, index) = wallet.derivation_of_spk(script_pubkey.clone()).unwrap();
            let redeem_script = wallet
                .public_descriptor(keychain)
                .at_derivation_index(index)
                .unwrap()
                .explicit_script()
                .unwrap();
            let push = bitcoin::script::PushBytesBuf::try_from(redeem_script.to_bytes()).unwrap();
            assert_eq!(
                input.final_script_sig,
                Some(ScriptBuf::builder().push_slice(push).into_script()),
                "BDK pushes the redeem script"
            );
        } else {
            assert!(input.final_script_witness.is_none());
            assert!(input.final_script_sig.is_none());
        }
    }
}

/// Every funding input, payout, and change output in the completed contract is
/// P2SH, as two BIP49 wallets produce.
fn assert_all_p2sh(funding_transaction: &Transaction, offer: &OfferDlc, accept: &AcceptDlc) {
    for input in &funding_transaction.input {
        assert!(
            !input.script_sig.is_empty(),
            "wrapped inputs push a redeem script"
        );
        assert_eq!(input.witness.len(), 2);
    }
    assert!(offer.payout_spk.is_p2sh());
    assert!(offer.change_spk.is_p2sh());
    assert!(accept.payout_spk.is_p2sh());
    assert!(accept.change_spk.is_p2sh());
    let change_outputs = funding_transaction
        .output
        .iter()
        .filter(|output| output.script_pubkey.is_p2sh())
        .count();
    assert_eq!(change_outputs, 2, "one P2SH change output per party");
}

#[test]
fn bip49_bdk_wallets_sign_both_sides_of_the_funding_psbt() {
    // Both parties bring their own BDK BIP49 wallet. Each wallet is given only
    // the funding PSBT from create_funding_psbt (witness UTXO, no BIP32
    // derivation info) and must sign and finalize its own P2SH-P2WPKH input.
    let secp = Secp256k1::new();
    let (offer_wallet, offerer) =
        PartySetup::bip49(&secp, 1, NETWORK, Amount::from_sat(150_000), 1);
    let (accept_wallet, accepter) =
        PartySetup::bip49(&secp, 2, NETWORK, Amount::from_sat(150_000), 2);
    let (offerer, _accepter, offer, accept) =
        enum_contract_between(&secp, NETWORK, offerer, accepter);

    let sign_options = SignOptions {
        trust_witness_utxo: true,
        ..Default::default()
    };

    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    let finalized = offer_wallet
        .sign(&mut offer_psbt, sign_options.clone())
        .unwrap();
    assert!(!finalized, "the accepter's input is still unsigned");
    assert_bdk_signed_own_inputs(&offer_psbt, &offer_wallet);
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    let finalized = accept_wallet.sign(&mut accept_psbt, sign_options).unwrap();
    assert!(!finalized, "the offerer's input is still unsigned");
    assert_bdk_signed_own_inputs(&accept_psbt, &accept_wallet);
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).unwrap();

    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
    assert_all_p2sh(&funding_transaction, &offer, &accept);
}

#[test]
fn bip49_descriptors_sign_both_sides_of_the_funding_psbt() {
    // The same two BIP49 wallets, but each side signs through the stateless
    // sh(wpkh()) descriptor path instead of handing the PSBT to BDK.
    let secp = Secp256k1::new();
    let (_, offerer) = PartySetup::bip49(&secp, 1, NETWORK, Amount::from_sat(150_000), 1);
    let (_, accepter) = PartySetup::bip49(&secp, 2, NETWORK, Amount::from_sat(150_000), 2);
    let (offerer, accepter, offer, accept) =
        enum_contract_between(&secp, NETWORK, offerer, accepter);

    let offer_descriptor = format!("sh(wpkh({}/49h/1h/0h/0/*))", offerer.xpriv);
    let mut offer_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut offer_psbt,
        &offer_descriptor,
        &[DescriptorInput {
            input_serial_id: offerer.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    )
    .unwrap();
    let sign_result =
        sign_accept(&offer, &accept, &offerer.funding_secret_key, &offer_psbt).unwrap();

    let accept_descriptor = format!("sh(wpkh({}/49h/1h/0h/0/*))", accepter.xpriv);
    let mut accept_psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_descriptor(
        &offer,
        &accept,
        &mut accept_psbt,
        &accept_descriptor,
        &[DescriptorInput {
            input_serial_id: accepter.funding_input.input_serial_id,
            derivation_index: 0,
        }],
    )
    .unwrap();
    let funding_transaction =
        finalize_sign(&offer, &accept, &sign_result.sign, &accept_psbt).unwrap();

    assert_funding_transaction_complete(&funding_transaction, &offer, &accept);
    assert_all_p2sh(&funding_transaction, &offer, &accept);
}

#[test]
fn truncated_counterparty_adaptor_signatures_are_rejected() {
    // A counterparty that sends fewer adaptor signatures than the contract
    // needs must get a protocol error, not panic the message handler.
    let secp = Secp256k1::new();
    let (offerer, _, offer, mut accept) = enum_contract(&secp, NETWORK);
    accept
        .cet_adaptor_signatures
        .ecdsa_adaptor_signatures
        .clear();

    let mut psbt = create_funding_psbt(&offer, &accept).unwrap();
    signing::sign_funding_psbt_with_xpriv(
        &offer,
        &accept,
        &mut psbt,
        &offerer.xpriv,
        &offerer.derivations(),
    )
    .unwrap();

    let error = sign_accept(&offer, &accept, &offerer.funding_secret_key, &psbt)
        .err()
        .expect("a truncated adaptor signature list is an invalid accept");
    match error {
        ContractError::InvalidAccept(message) => {
            assert!(message.contains("adaptor signature"), "{message}");
        }
        other => panic!("expected an invalid accept error, got {other:?}"),
    }
}

struct PreRc4Contract {
    offer: OfferDlc,
    accept: AcceptDlc,
    sign: SignDlc,
    offerer_key: SecretKey,
    accepter_key: SecretKey,
    funding_transaction: Transaction,
    refund_signed_by_accepter: Transaction,
    cet_up_signed_by_offerer: Transaction,
}

fn pre_rc4_single_funded_contract() -> PreRc4Contract {
    use ddk_messages::lightning::util::ser::Readable;
    let fields: std::collections::HashMap<&str, Vec<u8>> =
        include_str!("fixtures/pre_rc4_single_funded.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                let (name, hex) = line.split_once(' ').unwrap();
                let bytes = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                (name, bytes)
            })
            .collect();
    fn message<T: Readable>(bytes: &[u8]) -> T {
        T::read(&mut ddk_messages::lightning::io::Cursor::new(
            bytes.to_vec(),
        ))
        .unwrap()
    }
    let transaction = |name: &str| bitcoin::consensus::deserialize(&fields[name]).unwrap();
    PreRc4Contract {
        offer: message(&fields["offer"]),
        accept: message(&fields["accept"]),
        sign: message(&fields["sign"]),
        offerer_key: SecretKey::from_slice(&fields["offerer_funding_secret_key"]).unwrap(),
        accepter_key: SecretKey::from_slice(&fields["accepter_funding_secret_key"]).unwrap(),
        funding_transaction: transaction("funding_transaction"),
        refund_signed_by_accepter: transaction("refund_signed_by_accepter"),
        cet_up_signed_by_offerer: transaction("cet_up_signed_by_offerer"),
    }
}

#[test]
fn a_pre_rc4_single_funded_contract_rebuilds_differently_under_the_current_rule() {
    let contract = pre_rc4_single_funded_contract();
    let current = create_dlc_transactions(&contract.offer, &contract.accept).unwrap();
    assert_ne!(
        current.fund.compute_txid(),
        contract.funding_transaction.compute_txid()
    );
}

#[test]
fn a_pre_rc4_single_funded_contract_rebuilds_from_its_sign_message() {
    let contract = pre_rc4_single_funded_contract();
    let transactions =
        create_signed_dlc_transactions(&contract.offer, &contract.accept, &contract.sign).unwrap();
    assert_eq!(
        transactions.fund.compute_txid(),
        contract.funding_transaction.compute_txid()
    );
    assert_eq!(
        transactions.refund.compute_txid(),
        contract.refund_signed_by_accepter.compute_txid()
    );
}

#[test]
fn a_pre_rc4_single_funded_contract_settles_with_the_same_transactions() {
    let contract = pre_rc4_single_funded_contract();
    let (offer, accept, sign) = (&contract.offer, &contract.accept, &contract.sign);

    let refund = sign_refund(offer, accept, sign, &contract.accepter_key).unwrap();
    assert_eq!(refund, contract.refund_signed_by_accepter);
    let attestations = vec![(0, oracle_attestation(vec!["up".to_string()]))];
    let cet = sign_cet(offer, accept, sign, &contract.offerer_key, &attestations).unwrap();
    assert_eq!(cet, contract.cet_up_signed_by_offerer);

    let refund = sign_refund(offer, accept, sign, &contract.offerer_key).unwrap();
    assert_eq!(
        refund.compute_txid(),
        contract.refund_signed_by_accepter.compute_txid()
    );
    let cet = sign_cet(offer, accept, sign, &contract.accepter_key, &attestations).unwrap();
    assert_eq!(
        cet.compute_txid(),
        contract.cet_up_signed_by_offerer.compute_txid()
    );
}

#[test]
fn a_pre_rc4_single_funded_contract_can_be_spliced() {
    let contract = pre_rc4_single_funded_contract();
    let input = create_dlc_splice_input(
        &contract.offer,
        &contract.accept,
        &contract.sign,
        Party::Offer,
        Some(900),
        DLC_INPUT_MAX_WITNESS_LEN,
    )
    .unwrap();
    let prev_tx: Transaction = bitcoin::consensus::deserialize(&input.prev_tx).unwrap();
    assert_eq!(
        prev_tx.compute_txid(),
        contract.funding_transaction.compute_txid()
    );
    assert_eq!(
        input.dlc_input.unwrap().contract_id,
        contract.sign.contract_id
    );
}

#[test]
fn a_sign_message_that_matches_neither_rule_is_rejected() {
    let contract = pre_rc4_single_funded_contract();
    let mut sign = contract.sign.clone();
    sign.contract_id[0] ^= 1;
    assert!(matches!(
        create_signed_dlc_transactions(&contract.offer, &contract.accept, &sign),
        Err(ContractError::InvalidSign(_))
    ));
    assert!(matches!(
        sign_refund(
            &contract.offer,
            &contract.accept,
            &sign,
            &contract.accepter_key
        ),
        Err(ContractError::InvalidSign(_))
    ));
}

/// The contract shapes whose transactions are pinned in
/// `fixtures/contract_transactions/`: one offer/accept pair per shape the
/// transaction builder has to keep producing byte for byte.
///
/// Serial ids and temporary contract ids are random, so this list only feeds
/// the generator; the test replays the pinned messages.
fn transaction_fixture_contracts() -> Vec<(&'static str, OfferDlc, AcceptDlc)> {
    let secp = Secp256k1::new();
    let accept = |offer: &OfferDlc, accepter: &PartySetup, funding_inputs: Vec<FundingInput>| {
        accept_offer(
            offer,
            AcceptOfferParams {
                party: accepter.party_params(&secp, funding_inputs),
                min_timeout_interval: MIN_TIMEOUT,
                max_timeout_interval: MAX_TIMEOUT,
                now_unix: NOW_UNIX,
            },
            &accepter.funding_secret_key,
        )
        .unwrap()
        .accept
    };
    let half = Amount::from_sat(50_000);
    let offerer = PartySetup::new(&secp, 1, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 2, NETWORK, Amount::from_sat(150_000), 2);
    let dual_funded = |contract_info: ContractInfo, contract_flags: u8| {
        let mut params = offer_params(
            &secp,
            &offerer,
            contract_info,
            half,
            NETWORK,
            vec![offerer.funding_input.clone()],
        );
        params.contract_flags = contract_flags;
        let offer = create_offer(params).unwrap();
        let accept = accept(&offer, &accepter, vec![accepter.funding_input.clone()]);
        (offer, accept)
    };

    // Rounded to 10,000 sats, the numerical curve has a handful of CETs rather
    // than hundreds, which keeps the pinned accept messages small.
    let numerical = || numerical_contract_info_rounded(half, half, 10_000);

    let mut contracts = Vec::new();
    let (offer, accept_msg) = dual_funded(enum_contract_info(TOTAL_COLLATERAL), 0);
    contracts.push(("enum_dual_funded", offer, accept_msg));
    let (offer, accept_msg) = dual_funded(numerical(), 0);
    contracts.push(("numerical_dual_funded", offer, accept_msg));
    let (offer, accept_msg) = dual_funded(enum_contract_info(TOTAL_COLLATERAL), 1);
    contracts.push(("enum_refund_to_accepter", offer, accept_msg));

    // A disjoint contract: the CETs of every contract info after the first
    // are built separately from the first info's transactions.
    let ContractInfo::SingleContractInfo(enum_info) = enum_contract_info(TOTAL_COLLATERAL) else {
        unreachable!()
    };
    let ContractInfo::SingleContractInfo(numerical_info) = numerical() else {
        unreachable!()
    };
    let disjoint = ContractInfo::DisjointContractInfo(DisjointContractInfo {
        total_collateral: TOTAL_COLLATERAL,
        contract_infos: vec![enum_info.contract_info, numerical_info.contract_info],
    });
    let (offer, accept_msg) = dual_funded(disjoint, 0);
    contracts.push(("disjoint_enum_numerical", offer, accept_msg));

    // Single funded: the accepting party contributes no inputs, so the fee
    // rule decides what the offering party's input has to cover.
    let offerer = PartySetup::new(&secp, 31, NETWORK, Amount::from_sat(250_000), 1);
    let accepter = PartySetup::new(&secp, 32, NETWORK, Amount::from_sat(150_000), 2);
    let offer = create_offer(offer_params(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        TOTAL_COLLATERAL,
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    let accept_msg = accept(&offer, &accepter, vec![]);
    contracts.push(("enum_single_funded", offer, accept_msg));

    // Spliced: the offer spends a previous contract's funding output.
    let prepared = prepare_splice(true);
    contracts.push(("enum_spliced_in", prepared.offer_b, prepared.accept_b));
    contracts
}

/// Summarizes a contract's unsigned transactions. A txid covers every byte of
/// an unsigned transaction, so equal digests mean equal transactions.
fn transaction_digest(transactions: &ddk_dlc::DlcTransactions) -> serde_json::Value {
    use bitcoin::hashes::{sha256, Hash};
    let mut cet_bytes = Vec::new();
    for cet in &transactions.cets {
        cet_bytes.extend(bitcoin::consensus::serialize(cet));
    }
    serde_json::json!({
        "fund_txid": transactions.fund.compute_txid().to_string(),
        "refund_txid": transactions.refund.compute_txid().to_string(),
        "cet_count": transactions.cets.len(),
        "cets_sha256": sha256::Hash::hash(&cet_bytes).to_string(),
    })
}

/// The transactions `ddk::contract` built for every contract shape before the
/// builder moved into `ddk-manager`, replayed from the pinned messages.
///
/// Each shape is a wire-encoded offer and accept, `<shape>.offer.bin` and
/// `<shape>.accept.bin`, and its entry in `digests.json`. A stored or signed
/// contract is settled by rebuilding exactly these transactions, so a change
/// to any digest is a change for every existing contract of that shape. Set
/// `GENERATE_CONTRACT_TRANSACTION_FIXTURES` to rewrite the fixtures from the
/// current builder.
#[test]
fn contract_transactions_match_the_pinned_fixtures() {
    use ddk_messages::lightning::util::ser::{Readable, Writeable};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contract_transactions");
    let digests_path = dir.join("digests.json");
    if std::env::var_os("GENERATE_CONTRACT_TRANSACTION_FIXTURES").is_some() {
        let mut digests = serde_json::Map::new();
        for (name, offer, accept) in transaction_fixture_contracts() {
            std::fs::write(dir.join(format!("{name}.offer.bin")), offer.encode()).unwrap();
            std::fs::write(dir.join(format!("{name}.accept.bin")), accept.encode()).unwrap();
            let transactions = create_dlc_transactions(&offer, &accept).unwrap();
            digests.insert(name.to_string(), transaction_digest(&transactions));
        }
        let json = serde_json::to_string_pretty(&digests).unwrap();
        std::fs::write(&digests_path, json + "\n").unwrap();
        return;
    }

    let digests: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&digests_path).unwrap()).unwrap();
    assert!(!digests.is_empty());
    for (name, expected) in &digests {
        let message = |kind: &str| std::fs::read(dir.join(format!("{name}.{kind}.bin"))).unwrap();
        let offer = OfferDlc::read(&mut message("offer").as_slice()).unwrap();
        let accept = AcceptDlc::read(&mut message("accept").as_slice()).unwrap();
        let transactions = create_dlc_transactions(&offer, &accept).unwrap();
        assert_eq!(
            &transaction_digest(&transactions),
            expected,
            "{name}: transactions changed"
        );
    }
}

// ---------------------------------------------------------------------------
// Payout script overrides
//
// A record on the offer that makes the CETs of named outcomes, an enum
// outcome or a range of numeric values, pay a script in the accepting party's
// place. Both parties read it from the same
// offer, so the adaptor signatures of a contract that carries one verify only
// if both apply it.

/// A 22-byte P2WPKH script that belongs to neither party.
fn third_party_script() -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([0x42; 20]))
}

fn payout_overrides(overrides: &[(OverrideOutcome, ScriptBuf)]) -> PayoutScriptOverrides {
    PayoutScriptOverrides {
        overrides: overrides
            .iter()
            .map(|(outcome, script_pubkey)| PayoutScriptOverride {
                outcome: outcome.clone(),
                script_pubkey: script_pubkey.clone(),
            })
            .collect(),
    }
}

/// An enum outcome.
fn outcome(outcome: &str) -> OverrideOutcome {
    OverrideOutcome::Enum {
        outcome: outcome.to_string(),
    }
}

/// The numeric values from `start` to `end`, both included.
fn values(start: u64, end: u64) -> OverrideOutcome {
    OverrideOutcome::Numeric { start, end }
}

/// The values each CET of a single numerical contract info covers, in CET
/// order, with what it pays.
fn range_payouts(contract_info: &ContractInfo) -> Vec<ddk_dlc::RangePayout> {
    let execution_infos = ddk_manager::contract::execution_contract_infos(contract_info).unwrap();
    let ddk_manager::contract::ContractDescriptor::Numerical(descriptor) =
        &execution_infos[0].contract_descriptor
    else {
        panic!("not a numerical contract info");
    };
    descriptor
        .get_range_payouts(contract_info.get_total_collateral())
        .unwrap()
}

/// The CET range of `ranges` that covers `value`.
fn range_covering(ranges: &[ddk_dlc::RangePayout], value: u64) -> ddk_dlc::RangePayout {
    let value = value as usize;
    ranges
        .iter()
        .find(|range| range.start <= value && value < range.start + range.count)
        .expect("a CET covering the value")
        .clone()
}

/// An offer carrying `overrides`, over `contract_info`.
fn offer_with_overrides(
    secp: &Secp256k1<All>,
    offerer: &PartySetup,
    contract_info: ContractInfo,
    offer_collateral: Amount,
    overrides: &PayoutScriptOverrides,
) -> OfferDlc {
    let mut offer = create_offer(offer_params(
        secp,
        offerer,
        contract_info,
        offer_collateral,
        NETWORK,
        vec![offerer.funding_input.clone()],
    ))
    .unwrap();
    offer.tlvs.set(overrides);
    offer
}

fn accept_with_params(
    offer: &OfferDlc,
    accepter: &PartySetup,
    party: PartyParams,
) -> Result<AcceptDlc, ContractError> {
    accept_offer(
        offer,
        AcceptOfferParams {
            party,
            min_timeout_interval: MIN_TIMEOUT,
            max_timeout_interval: MAX_TIMEOUT,
            now_unix: NOW_UNIX,
        },
        &accepter.funding_secret_key,
    )
    .map(|result| result.accept)
}

fn validate(offer: &OfferDlc) -> Result<(), ContractError> {
    validate_offer(offer, MIN_TIMEOUT, MAX_TIMEOUT, NOW_UNIX)
}

fn output_value(transaction: &Transaction, script_pubkey: &ScriptBuf) -> Option<Amount> {
    transaction
        .output
        .iter()
        .find(|output| output.script_pubkey == *script_pubkey)
        .map(|output| output.value)
}

#[test]
fn a_payout_script_override_pays_the_named_outcome_to_the_script() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 71, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 72, NETWORK, Amount::from_sat(150_000), 2);
    let overrides = payout_overrides(&[(outcome("down"), third_party_script())]);
    let offer = offer_with_overrides(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        &overrides,
    );
    assert!(validate(&offer).is_ok());
    let accept = accept_with_params(
        &offer,
        &accepter,
        accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
    )
    .unwrap();
    // Each side verifies the other's adaptor signatures over the CETs it
    // built itself, so funding succeeds only if both applied the record.
    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    for key in [&offerer.funding_secret_key, &accepter.funding_secret_key] {
        let down = sign_cet(
            &offer,
            &accept,
            &sign,
            key,
            &[(0, oracle_attestation(vec!["down".to_string()]))],
        )
        .unwrap();
        assert_spends_funding_output(&down, &offer, &accept, &funding_transaction);
        assert_eq!(
            output_value(&down, &third_party_script()),
            Some(TOTAL_COLLATERAL)
        );
        assert_eq!(output_value(&down, &accept.payout_spk), None);

        let up = sign_cet(
            &offer,
            &accept,
            &sign,
            key,
            &[(0, oracle_attestation(vec!["up".to_string()]))],
        )
        .unwrap();
        assert_spends_funding_output(&up, &offer, &accept, &funding_transaction);
        assert_eq!(output_value(&up, &offer.payout_spk), Some(TOTAL_COLLATERAL));
        assert_eq!(output_value(&up, &third_party_script()), None);
    }
}

/// Both parties may pay out to the same script. The record replaces the
/// accepting party's output, not whichever output happens to pay that script.
#[test]
fn a_payout_script_override_replaces_only_the_accepting_partys_output() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 73, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 74, NETWORK, Amount::from_sat(150_000), 2);
    let overrides = payout_overrides(&[(outcome("up"), third_party_script())]);
    let offer = offer_with_overrides(
        &secp,
        &offerer,
        enum_contract_info_paying(TOTAL_COLLATERAL, Amount::from_sat(60_000)),
        Amount::from_sat(50_000),
        &overrides,
    );
    let mut party = accepter.party_params(&secp, vec![accepter.funding_input.clone()]);
    party.payout_spk = offer.payout_spk.clone();
    let accept = accept_with_params(&offer, &accepter, party).unwrap();
    assert_eq!(accept.payout_spk, offer.payout_spk);

    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);
    let up = sign_cet(
        &offer,
        &accept,
        &sign,
        &accepter.funding_secret_key,
        &[(0, oracle_attestation(vec!["up".to_string()]))],
    )
    .unwrap();
    assert_spends_funding_output(&up, &offer, &accept, &funding_transaction);
    assert_eq!(up.output.len(), 2);
    assert_eq!(
        output_value(&up, &offer.payout_spk),
        Some(Amount::from_sat(60_000))
    );
    assert_eq!(
        output_value(&up, &third_party_script()),
        Some(Amount::from_sat(40_000))
    );
}

#[test]
fn a_payout_script_override_the_contract_cannot_carry_is_an_invalid_offer() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 75, NETWORK, Amount::from_sat(150_000), 1);
    let half = Amount::from_sat(50_000);
    let invalid_offers = [
        (
            "an outcome no enum descriptor has",
            enum_contract_info(TOTAL_COLLATERAL),
            payout_overrides(&[(outcome("sideways"), third_party_script())]),
        ),
        (
            "an empty script",
            enum_contract_info(TOTAL_COLLATERAL),
            payout_overrides(&[(outcome("down"), ScriptBuf::new())]),
        ),
        (
            "the same outcome twice",
            enum_contract_info(TOTAL_COLLATERAL),
            payout_overrides(&[
                (outcome("down"), third_party_script()),
                (outcome("down"), offerer.payout_spk.clone()),
            ]),
        ),
        (
            "an enum outcome on a numerical contract",
            numerical_contract_info(half, half),
            payout_overrides(&[(outcome("down"), third_party_script())]),
        ),
        (
            "numeric values on an enum contract",
            enum_contract_info(TOTAL_COLLATERAL),
            payout_overrides(&[(values(0, 1), third_party_script())]),
        ),
        (
            "numeric values past every CET",
            numerical_contract_info(half, half),
            payout_overrides(&[(values(5_000, 6_000), third_party_script())]),
        ),
        (
            "numeric values starting after they end",
            numerical_contract_info(half, half),
            payout_overrides(&[(values(500, 400), third_party_script())]),
        ),
        (
            "numeric values covering part of a CET",
            numerical_contract_info(half, half),
            {
                // A CET over several values, overridden for its first value
                // only: its other values would pay the accepting party.
                let ranges = range_payouts(&numerical_contract_info(half, half));
                let wide = ranges
                    .iter()
                    .find(|range| range.count > 1)
                    .expect("a CET covering more than one value");
                let first = wide.start as u64;
                payout_overrides(&[(values(first, first), third_party_script())])
            },
        ),
        (
            "two numeric ranges covering the same CET",
            numerical_contract_info(half, half),
            {
                let at = range_covering(&range_payouts(&numerical_contract_info(half, half)), 500);
                let (first, last) = (at.start as u64, (at.start + at.count - 1) as u64);
                payout_overrides(&[
                    (values(first, last), third_party_script()),
                    (values(first, last), offerer.payout_spk.clone()),
                ])
            },
        ),
    ];
    for (reason, contract_info, overrides) in invalid_offers {
        let offer = offer_with_overrides(&secp, &offerer, contract_info, half, &overrides);
        assert!(
            matches!(validate(&offer), Err(ContractError::InvalidOffer(_))),
            "{reason}"
        );
    }
}

/// A numeric range overrides every CET whose values it covers, and no other.
#[test]
fn a_numeric_payout_script_override_pays_the_covered_values_to_the_script() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 79, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 80, NETWORK, Amount::from_sat(150_000), 2);
    let half = Amount::from_sat(50_000);
    let contract_info = numerical_contract_info(half, half);
    let ranges = range_payouts(&contract_info);
    let covered = range_covering(&ranges, 500);
    let uncovered = range_covering(&ranges, 300);
    assert!(covered.payout.accept > Amount::ZERO && uncovered.payout.accept > Amount::ZERO);
    let overrides = payout_overrides(&[(
        values(
            covered.start as u64,
            (covered.start + covered.count - 1) as u64,
        ),
        third_party_script(),
    )]);
    let offer = offer_with_overrides(&secp, &offerer, contract_info, half, &overrides);
    assert!(validate(&offer).is_ok());
    let accept = accept_with_params(
        &offer,
        &accepter,
        accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
    )
    .unwrap();
    // Each side verifies the other's adaptor signatures over the CETs it
    // built itself, so funding succeeds only if both applied the record.
    let (sign, funding_transaction) = fund_with_xpriv(&secp, &offerer, &accepter, &offer, &accept);

    for key in [&offerer.funding_secret_key, &accepter.funding_secret_key] {
        let settle = |value: u64| {
            sign_cet(
                &offer,
                &accept,
                &sign,
                key,
                &[(0, oracle_attestation(digit_outcomes(value, 10)))],
            )
            .unwrap()
        };

        let overridden = settle(500);
        assert_spends_funding_output(&overridden, &offer, &accept, &funding_transaction);
        assert_eq!(
            output_value(&overridden, &third_party_script()),
            Some(covered.payout.accept)
        );
        assert_eq!(output_value(&overridden, &accept.payout_spk), None);
        assert_eq!(
            output_value(&overridden, &offer.payout_spk),
            Some(covered.payout.offer)
        );

        let untouched = settle(300);
        assert_spends_funding_output(&untouched, &offer, &accept, &funding_transaction);
        assert_eq!(
            output_value(&untouched, &accept.payout_spk),
            Some(uncovered.payout.accept)
        );
        assert_eq!(output_value(&untouched, &third_party_script()), None);
    }
}

/// This crate reads the first record of a type and node-dlc the last, so an
/// offer with two of them could not mean the same contract to both peers.
#[test]
fn an_offer_with_two_payout_script_override_records_is_invalid() {
    use ddk_messages::lightning::io::Cursor;
    use ddk_messages::lightning::util::ser::Writeable;
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 76, NETWORK, Amount::from_sat(150_000), 1);
    let mut offer = offer_with_overrides(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        &payout_overrides(&[(outcome("down"), third_party_script())]),
    );
    assert!(validate(&offer).is_ok());
    let mut twice = offer.tlvs.encode();
    twice.extend(offer.tlvs.encode());
    offer.tlvs = ddk_messages::TlvStream::read_to_end(&mut Cursor::new(twice)).unwrap();
    assert!(matches!(
        validate(&offer),
        Err(ContractError::InvalidOffer(_))
    ));
}

/// The funding transaction reserves the CET fee from the accepting party's
/// own payout script, so a longer override would underpay the fee rate.
#[test]
fn a_payout_script_override_longer_than_the_accept_script_is_an_invalid_accept() {
    let secp = Secp256k1::new();
    let offerer = PartySetup::new(&secp, 77, NETWORK, Amount::from_sat(150_000), 1);
    let accepter = PartySetup::new(&secp, 78, NETWORK, Amount::from_sat(150_000), 2);
    let p2wsh = ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([0x42; 32]));
    let offer = offer_with_overrides(
        &secp,
        &offerer,
        enum_contract_info(TOTAL_COLLATERAL),
        Amount::from_sat(50_000),
        &payout_overrides(&[(outcome("down"), p2wsh)]),
    );
    // Nothing about the offer alone is wrong: it is the 22-byte P2WPKH payout
    // script this accepting party brings that the 34-byte override exceeds.
    assert!(validate(&offer).is_ok());
    let result = accept_with_params(
        &offer,
        &accepter,
        accepter.party_params(&secp, vec![accepter.funding_input.clone()]),
    );
    assert!(matches!(result, Err(ContractError::InvalidAccept(_))));
}

#[test]
fn an_accept_whose_collateral_does_not_complete_the_total_is_rejected() {
    let secp = Secp256k1::new();
    let (_, _, offer, mut accept) = enum_contract(&secp, NETWORK);
    accept.accept_collateral += Amount::ONE_SAT;
    assert!(matches!(
        create_dlc_transactions(&offer, &accept),
        Err(ContractError::InvalidAccept(_))
    ));
}

#[test]
fn an_accept_whose_collateral_overflows_the_total_is_rejected() {
    let secp = Secp256k1::new();
    let (_, _, offer, mut accept) = enum_contract(&secp, NETWORK);
    accept.accept_collateral = Amount::MAX;
    assert!(matches!(
        create_dlc_transactions(&offer, &accept),
        Err(ContractError::InvalidAccept(_))
    ));
}

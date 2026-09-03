//! Lifecycle tests for the stateless contract API.
//!
//! Every test completes (or rejects) a DLC using only wire messages, explicit
//! party data, and PSBTs — no storage backend, contract manager, or
//! blockchain client is constructed anywhere in this file.

use bitcoin::absolute::LockTime;
use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::psbt::Psbt;
use bitcoin::transaction::Version;
use bitcoin::{Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use ddk::contract::{
    accept_offer, chain_hash_from_network, create_dlc_splice_input, create_dlc_transactions,
    create_funding_psbt, create_offer, finalize_sign, finalize_sign_spliced, funding_input,
    sign_accept, sign_accept_spliced, sign_cet, sign_refund, signing, AcceptOfferParams,
    ContractError, CreateOfferParams, DescriptorInput, DlcInputSigningKey, InputDerivation, Party,
    PartyParams, DLC_INPUT_MAX_WITNESS_LEN,
};
use ddk_dlc::secp256k1_zkp::{All, Keypair, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey};
use ddk_messages::contract_msgs::{
    ContractDescriptor, ContractInfo, ContractInfoInner, ContractOutcome,
    EnumeratedContractDescriptor, NumericOutcomeContractDescriptor, SingleContractInfo,
};
use ddk_messages::oracle_msgs::{
    tagged_announcement_msg, tagged_attestation_msg, DigitDecompositionEventDescriptor,
    EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleAttestation, OracleEvent,
    OracleInfo, SingleOracleInfo,
};
use ddk_messages::{AcceptDlc, FundingInput, OfferDlc, SignDlc, WitnessElement};
use std::str::FromStr;

const NETWORK: Network = Network::Regtest;
const MIN_TIMEOUT: u32 = 100;
const MAX_TIMEOUT: u32 = 500;
const TOTAL_COLLATERAL: Amount = Amount::from_sat(100_000);

/// One side of a contract: a DLC funding key plus a BIP84 wallet key
/// controlling a single funding UTXO.
struct PartySetup {
    funding_secret_key: SecretKey,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    funding_input: FundingInput,
}

impl PartySetup {
    fn new(
        secp: &Secp256k1<All>,
        seed_byte: u8,
        network: Network,
        utxo_value: Amount,
        input_serial_id: u64,
    ) -> Self {
        let funding_secret_key = SecretKey::from_slice(&[seed_byte; 32]).unwrap();
        let xpriv = Xpriv::new_master(network, &[seed_byte.wrapping_add(100); 64]).unwrap();
        let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
        let derivation_path =
            DerivationPath::from_str(&format!("84h/{coin_type}h/0h/0/0")).unwrap();
        let script_pubkey = p2wpkh_script(secp, &xpriv, &derivation_path);
        let previous_transaction = previous_transaction(utxo_value, script_pubkey);
        let funding_input = funding_input(
            &previous_transaction,
            0,
            Some(input_serial_id),
            u32::MAX,
            108,
            ScriptBuf::new(),
        )
        .unwrap();
        Self {
            funding_secret_key,
            xpriv,
            derivation_path,
            funding_input,
        }
    }

    fn funding_pubkey(&self, secp: &Secp256k1<All>) -> PublicKey {
        self.funding_secret_key.public_key(secp)
    }

    fn payout_script(&self, secp: &Secp256k1<All>) -> ScriptBuf {
        p2wpkh_script(secp, &self.xpriv, &self.derivation_path)
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

fn enum_contract_info(total_collateral: Amount) -> ContractInfo {
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
                            offer_payout: total_collateral,
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
                rounding_mod: 1,
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
        cet_locktime: 750,
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
        assert_eq!(
            prevout.script_pubkey,
            ScriptBuf::new_p2wpkh(&public_key.wpubkey_hash().unwrap()),
            "witness key does not control the spent output"
        );
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
        },
        &accepter.funding_secret_key,
    )
    .unwrap();
    complete_with_xpriv(&secp, &offerer, &accepter, &offer, &accept_result.accept);
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
    let funding_tx_a = complete_with_xpriv(&secp, &offerer_a, &accepter_a, &offer_a, &accept_a);
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

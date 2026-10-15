//! Unit tests for internal contract helpers. The full lifecycle scenarios
//! live in `ddk/tests/stateless.rs` and exercise only the public API.

use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::psbt::Psbt;
use bitcoin::transaction::Version;
use bitcoin::{Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use ddk_dlc::secp256k1_zkp::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use ddk_messages::contract_msgs::{
    ContractDescriptor, ContractInfo, ContractInfoInner, ContractOutcome,
    EnumeratedContractDescriptor, SingleContractInfo,
};
use ddk_messages::oracle_msgs::{
    tagged_announcement_msg, EnumEventDescriptor, EventDescriptor, OracleAnnouncement, OracleEvent,
    OracleInfo, SingleOracleInfo,
};
use ddk_messages::{
    AcceptDlc, CetAdaptorSignatures, DlcInput, FundingInput, FundingSignatures, OfferDlc, SignDlc,
};

use super::context::{
    contract_id_from_transactions, dlc_party_params, ensure_no_dlc_inputs, funding_input_index,
    validate_offer_funding_inputs,
};
use super::psbt::finalize_segwit_input;
use super::types::{funding_input, network_from_chain_hash, random_serial_id};
use super::*;

fn dummy_transaction(value: Amount, script_pubkey: ScriptBuf) -> Transaction {
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

fn dummy_funding_input(serial_id: u64) -> FundingInput {
    let script = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([7; 20]));
    funding_input(
        &dummy_transaction(Amount::from_sat(10_000), script),
        0,
        Some(serial_id),
        u32::MAX,
        108,
        ScriptBuf::new(),
    )
    .unwrap()
}

fn enum_contract_info() -> ContractInfo {
    let secp = Secp256k1::new();
    let oracle_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[8; 32]).unwrap());
    let nonce_key = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[9; 32]).unwrap());
    let oracle_event = OracleEvent {
        oracle_nonces: vec![XOnlyPublicKey::from_keypair(&nonce_key).0],
        event_maturity_epoch: 750,
        event_descriptor: EventDescriptor::EnumEvent(EnumEventDescriptor {
            outcomes: vec!["up".to_string(), "down".to_string()],
        }),
        event_id: "unit-test".to_string(),
    };
    let announcement = OracleAnnouncement {
        announcement_signature: secp
            .sign_schnorr(&tagged_announcement_msg(&oracle_event), &oracle_key),
        oracle_public_key: XOnlyPublicKey::from_keypair(&oracle_key).0,
        oracle_event,
    };
    ContractInfo::SingleContractInfo(SingleContractInfo {
        total_collateral: Amount::from_sat(100_000),
        contract_info: ContractInfoInner {
            contract_descriptor: ContractDescriptor::EnumeratedContractDescriptor(
                EnumeratedContractDescriptor {
                    payouts: vec![
                        ContractOutcome {
                            outcome: "up".to_string(),
                            offer_payout: Amount::from_sat(100_000),
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

fn messages_with_serial_ids(offer_ids: &[u64], accept_ids: &[u64]) -> (OfferDlc, AcceptDlc) {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_slice(&[1; 32]).unwrap();
    let public_key = secret_key.public_key(&secp);
    let script = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([7; 20]));
    let signature = secp.sign_ecdsa(&Message::from_digest([1; 32]), &secret_key);
    let offer = OfferDlc {
        protocol_version: PROTOCOL_VERSION,
        contract_flags: 0,
        chain_hash: chain_hash_from_network(Network::Regtest),
        temporary_contract_id: [42; 32],
        contract_info: enum_contract_info(),
        funding_pubkey: public_key,
        payout_spk: script.clone(),
        payout_serial_id: 1,
        offer_collateral: Amount::from_sat(50_000),
        funding_inputs: offer_ids
            .iter()
            .map(|id| dummy_funding_input(*id))
            .collect(),
        change_spk: script.clone(),
        change_serial_id: 2,
        fund_output_serial_id: 3,
        fee_rate_per_vb: 2,
        cet_locktime: 750,
        refund_locktime: 1_000,
        tlvs: Default::default(),
    };
    let accept = AcceptDlc {
        protocol_version: PROTOCOL_VERSION,
        temporary_contract_id: [42; 32],
        accept_collateral: Amount::from_sat(50_000),
        funding_pubkey: public_key,
        payout_spk: script.clone(),
        payout_serial_id: 4,
        funding_inputs: accept_ids
            .iter()
            .map(|id| dummy_funding_input(*id))
            .collect(),
        change_spk: script,
        change_serial_id: 5,
        cet_adaptor_signatures: CetAdaptorSignatures::from(&[][..]),
        refund_signature: signature,
        negotiation_fields: None,
        tlvs: Default::default(),
    };
    (offer, accept)
}

/// Builds a DLC (splice) funding input whose previous output is the 2-of-2 of
/// two fixed funding keys, unless `correct_script` is false.
fn dlc_funding_input(
    max_witness_len: u16,
    redeem_script: ScriptBuf,
    correct_script: bool,
) -> FundingInput {
    let secp = Secp256k1::new();
    let local_fund_pubkey = SecretKey::from_slice(&[3; 32]).unwrap().public_key(&secp);
    let remote_fund_pubkey = SecretKey::from_slice(&[4; 32]).unwrap().public_key(&secp);
    let script_pubkey = if correct_script {
        ddk_dlc::make_funding_redeemscript(&local_fund_pubkey, &remote_fund_pubkey).to_p2wsh()
    } else {
        ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([7; 20]))
    };
    let previous_transaction = dummy_transaction(Amount::from_sat(100_000), script_pubkey);
    FundingInput {
        input_serial_id: 9,
        prev_tx: bitcoin::consensus::serialize(&previous_transaction),
        prev_tx_vout: 0,
        sequence: u32::MAX,
        max_witness_len,
        redeem_script,
        dlc_input: Some(DlcInput {
            local_fund_pubkey,
            remote_fund_pubkey,
            contract_id: [5; 32],
        }),
    }
}

#[test]
fn validate_offer_funding_inputs_accepts_a_valid_dlc_input() {
    let input = dlc_funding_input(220, ScriptBuf::new(), true);
    assert!(validate_offer_funding_inputs(&[input]).is_ok());
}

#[test]
fn validate_offer_funding_inputs_rejects_malformed_dlc_inputs() {
    // Witness length too small to be a 2-of-2.
    assert!(matches!(
        validate_offer_funding_inputs(&[dlc_funding_input(108, ScriptBuf::new(), true)]),
        Err(ContractError::InvalidFundingInput(_))
    ));
    // A DLC input must not carry a redeem script.
    let redeem = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([1; 20]));
    assert!(matches!(
        validate_offer_funding_inputs(&[dlc_funding_input(220, redeem, true)]),
        Err(ContractError::InvalidFundingInput(_))
    ));
    // Previous output is not the 2-of-2 of the named funding keys.
    assert!(matches!(
        validate_offer_funding_inputs(&[dlc_funding_input(220, ScriptBuf::new(), false)]),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

#[test]
fn ensure_no_dlc_inputs_rejects_a_dlc_input() {
    assert!(matches!(
        ensure_no_dlc_inputs(&[dlc_funding_input(220, ScriptBuf::new(), true)]),
        Err(ContractError::InvalidFundingInput(_))
    ));
    // Ordinary inputs are permitted.
    assert!(ensure_no_dlc_inputs(&[dummy_funding_input(1)]).is_ok());
}

/// A single funded splice offer that also spends an ordinary P2WSH input of
/// 330 sats, whose witness is larger than a P2WPKH witness. Returns the
/// messages and the P2WSH input's outpoint.
fn splice_with_a_p2wsh_input() -> (OfferDlc, AcceptDlc, OutPoint) {
    let (mut offer, mut accept) = messages_with_serial_ids(&[1], &[]);
    offer.offer_collateral = Amount::from_sat(100_000);
    accept.accept_collateral = Amount::ZERO;
    let script = ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([3; 32]));
    let previous_transaction = dummy_transaction(Amount::from_sat(330), script);
    let p2wsh_outpoint = OutPoint::new(previous_transaction.compute_txid(), 0);
    let p2wsh_input = funding_input(
        &previous_transaction,
        0,
        Some(7),
        u32::MAX,
        200,
        ScriptBuf::new(),
    )
    .unwrap();
    offer
        .funding_inputs
        .extend([dlc_funding_input(220, ScriptBuf::new(), true), p2wsh_input]);
    (offer, accept, p2wsh_outpoint)
}

#[test]
fn a_new_splice_spends_its_p2wsh_input() {
    let (offer, accept, p2wsh_outpoint) = splice_with_a_p2wsh_input();
    let fund = create_dlc_transactions(&offer, &accept).unwrap().fund;
    let mut spent = fund.input.iter().map(|input| input.previous_output);
    assert_eq!(fund.input.len(), 3, "the splice input is spent once");
    assert!(spent.any(|outpoint| outpoint == p2wsh_outpoint));
}

/// ddk-dlc 2.0.0-rc.8 and earlier left a splice's P2WSH input out of the
/// funding transaction. With 330 sats, less than the funding fee, that
/// transaction was still valid, so a signed contract built that way must
/// rebuild exactly.
#[test]
fn a_splice_built_with_the_legacy_layout_rebuilds_from_its_sign_message() {
    let (offer, accept, p2wsh_outpoint) = splice_with_a_p2wsh_input();
    // What the spliced constructor of ddk-dlc 2.0.0-rc.8 built.
    let offer_params = dlc_party_params(
        offer.funding_pubkey,
        offer.payout_spk.clone(),
        offer.payout_serial_id,
        offer.change_spk.clone(),
        offer.change_serial_id,
        offer.offer_collateral,
        &offer.funding_inputs,
    )
    .unwrap();
    let accept_params = dlc_party_params(
        accept.funding_pubkey,
        accept.payout_spk.clone(),
        accept.payout_serial_id,
        accept.change_spk.clone(),
        accept.change_serial_id,
        accept.accept_collateral,
        &accept.funding_inputs,
    )
    .unwrap();
    let total = offer.get_total_collateral();
    let legacy = ddk_dlc::create_dlc_transactions(
        &offer_params.legacy_spliced(),
        &accept_params.legacy_spliced(),
        &[
            ddk_dlc::Payout {
                offer: total,
                accept: Amount::ZERO,
            },
            ddk_dlc::Payout {
                offer: Amount::ZERO,
                accept: total,
            },
        ],
        offer.refund_locktime,
        offer.fee_rate_per_vb,
        0,
        offer.cet_locktime,
        offer.fund_output_serial_id,
        offer.contract_flags,
    )
    .unwrap();
    let spent_value = offer_params.input_amount - Amount::from_sat(330);
    let output_value = legacy.fund.output.iter().map(|output| output.value).sum();
    assert!(spent_value > output_value, "the legacy funding pays a fee");
    assert!(legacy
        .fund
        .input
        .iter()
        .all(|input| input.previous_output != p2wsh_outpoint));
    assert_ne!(
        create_dlc_transactions(&offer, &accept).unwrap().fund,
        legacy.fund
    );

    let secp = Secp256k1::new();
    let sign = SignDlc {
        protocol_version: PROTOCOL_VERSION,
        contract_id: contract_id_from_transactions(&legacy, &offer.temporary_contract_id),
        cet_adaptor_signatures: CetAdaptorSignatures::from(&[][..]),
        refund_signature: secp.sign_ecdsa(
            &Message::from_digest([1; 32]),
            &SecretKey::from_slice(&[1; 32]).unwrap(),
        ),
        funding_signatures: FundingSignatures {
            funding_signatures: vec![],
        },
        tlvs: Default::default(),
    };
    let rebuilt = create_signed_dlc_transactions(&offer, &accept, &sign).unwrap();
    assert_eq!(rebuilt.fund, legacy.fund);
    assert_eq!(rebuilt.refund, legacy.refund);
    assert_eq!(rebuilt.cets, legacy.cets);
}

#[test]
fn funding_input_index_orders_by_serial_id() {
    let (offer, accept) = messages_with_serial_ids(&[50, 3], &[12]);
    assert_eq!(funding_input_index(&offer, &accept, 3).unwrap(), 0);
    assert_eq!(funding_input_index(&offer, &accept, 12).unwrap(), 1);
    assert_eq!(funding_input_index(&offer, &accept, 50).unwrap(), 2);
}

#[test]
fn funding_input_index_rejects_duplicates_and_unknown_ids() {
    let (offer, accept) = messages_with_serial_ids(&[5, 5], &[]);
    assert!(matches!(
        funding_input_index(&offer, &accept, 5),
        Err(ContractError::InvalidFundingInput(_))
    ));
    let (offer, accept) = messages_with_serial_ids(&[1], &[2]);
    assert!(matches!(
        funding_input_index(&offer, &accept, 9),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

#[test]
fn network_round_trips_through_chain_hash() {
    for network in [
        Network::Bitcoin,
        Network::Testnet,
        Network::Signet,
        Network::Regtest,
    ] {
        assert_eq!(
            network_from_chain_hash(chain_hash_from_network(network)),
            Some(network)
        );
    }
    assert_eq!(network_from_chain_hash([0; 32]), None);
}

#[test]
fn funding_input_rejects_missing_vout_and_bad_redeem_script() {
    let script = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([7; 20]));
    let transaction = dummy_transaction(Amount::from_sat(1_000), script.clone());
    assert!(matches!(
        funding_input(&transaction, 4, None, u32::MAX, 108, ScriptBuf::new()),
        Err(ContractError::InvalidFundingInput(_))
    ));
    // Redeem script for a non-P2SH output.
    assert!(matches!(
        funding_input(&transaction, 0, None, u32::MAX, 108, script),
        Err(ContractError::InvalidFundingInput(_))
    ));
}

#[test]
fn random_serial_ids_differ() {
    assert_ne!(random_serial_id(), random_serial_id());
}

#[test]
fn finalize_rejects_unsupported_script_types() {
    let script_pubkey = ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([9; 32]));
    let unsigned = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![],
    };
    let mut psbt = Psbt::from_unsigned_tx(unsigned).unwrap();
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(1_000),
        script_pubkey,
    });
    assert!(matches!(
        finalize_segwit_input(&mut psbt, 0),
        Err(ContractError::UnsupportedScriptType { input_index: 0 })
    ));
}

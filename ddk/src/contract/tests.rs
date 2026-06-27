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
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, FundingInput, OfferDlc};

use super::context::funding_input_index;
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
        cet_locktime: 500,
        refund_locktime: 1_000,
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
    };
    (offer, accept)
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

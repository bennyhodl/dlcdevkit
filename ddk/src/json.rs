//! JSON conversion utilities for DLC contracts
//!
//! This module provides functionality to convert Rust-DLC contract types into JSON
//! representations, primarily for console output and API responses. The conversion
//! is currently one-way (to JSON only) as the underlying Rust-DLC types don't
//! implement Serialize/Deserialize.
//!
//! # Current Usage
//! - Command line display of contract states
//! - API responses for contract information
//! - Debug output and logging
//!
//! # TODO: Future Improvements
//! - Create proper serializable/deserializable types that mirror Rust-DLC contracts
//! - Implement bidirectional conversion between JSON and contract types
//! - Add validation for contract state transitions
//! - Support custom serialization formats
//! - Add schema validation for JSON output

use ddk_manager::contract::{
    accepted_contract::AcceptedContract, offered_contract::OfferedContract,
    signed_contract::SignedContract, ClosedContract, Contract, FailedAcceptContract,
    FailedSignContract, PreClosedContract,
};
use ddk_messages::oracle_msgs::EventDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Represents an oracle event for JSON serialization.
/// Used to track event IDs, oracle public keys, and event types.
#[derive(Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct OracleEvent {
    pub event_id: String,
    pub oracle_pubkey: String,
    pub event_type: String,
}

/// Converts an offered contract to a JSON value with basic contract information.
/// Includes contract ID, party information, collateral, and associated oracle events.
pub fn offered_contract_to_value(offered_contract: &OfferedContract, state: &str) -> Value {
    let contract_id = hex::encode(offered_contract.id);
    let mut event_ids = HashSet::new();
    for contract_input in &offered_contract.contract_info {
        for announcement in &contract_input.oracle_announcements {
            let event_type = match announcement.oracle_event.event_descriptor {
                EventDescriptor::EnumEvent(_) => "enum",
                EventDescriptor::DigitDecompositionEvent(_) => "numerical",
            };
            let oracle_event = OracleEvent {
                event_id: announcement.oracle_event.event_id.to_string(),
                oracle_pubkey: announcement.oracle_public_key.to_string(),
                event_type: event_type.to_string(),
            };
            event_ids.insert(oracle_event);
        }
    }
    let event_ids = event_ids.into_iter().collect::<Vec<OracleEvent>>();
    json!({
        "state": state,
        "contract_id": contract_id,
        "is_offer_party": offered_contract.is_offer_party,
        "counter_party": offered_contract.counter_party.to_string(),
        "collateral": offered_contract.total_collateral,
        "offer_amount": offered_contract.offer_params.input_amount,
        "event_ids": event_ids,
    })
}

/// Converts an accepted contract to a JSON value, adding acceptance-specific details
/// such as transaction IDs and CET (Contract Execution Transaction) information.
fn accepted_contract_to_value(accepted: &AcceptedContract) -> Value {
    let offered_contract = offered_contract_to_value(&accepted.offered_contract, "offered");
    json!({
        "contract_id": hex::encode(accepted.offered_contract.id),
        "is_offer_party": offered_contract["is_offer_party"],
        "counter_party": offered_contract["counter_party"],
        "collateral": offered_contract["collateral"],
        "event_ids": offered_contract["event_ids"],
        "offer_amount": offered_contract["offer_amount"],
        "accept_amount": accepted.offered_contract.offer_params.input_amount,
        "num_cets": accepted.dlc_transactions.cets.len(),
        "funding_txid": accepted.dlc_transactions.fund.compute_txid(),
        "refund_txid": accepted.dlc_transactions.refund.compute_txid(),
    })
}

/// Converts a signed contract to a JSON value, including funding transaction
/// and contract state information.
pub fn signed_contract_to_value(signed: &SignedContract, state: &str) -> Value {
    let accepted_contract = accepted_contract_to_value(&signed.accepted_contract);
    json!({
        "state": state,
        "contract_id": accepted_contract["contract_id"],
        "counterparty": accepted_contract["counter_party"],
        "collateral": accepted_contract["collateral"],
        "event_ids": accepted_contract["event_ids"],
        "funding_txid": accepted_contract["funding_txid"],
    })
}

/// Converts a closed contract to a JSON value, including final state,
/// profit/loss information, and attestations.
fn closed_contract_to_value(closed: &ClosedContract) -> Value {
    json!({
        "state": "closed",
        "contract_id": hex::encode(closed.contract_id),
        "counterparty": closed.counter_party_id.to_string(),
        "pnl": closed.pnl.to_sat(),
        "signed_cet": closed.signed_cet,
        "attestations": closed.attestations,
    })
}

/// Converts a pre-closed contract to a JSON value, including attestations
/// and signed CET (Contract Execution Transaction) information.
pub fn preclosed_contract_to_value(preclosed: &PreClosedContract) -> Value {
    let signed_contract = signed_contract_to_value(&preclosed.signed_contract, "confirmed");
    json!({
        "state": "preclosed",
        "attestations": preclosed.attestations,
        "signed_cet_txid": preclosed.signed_cet.compute_txid(),
        "contract_id": signed_contract["contract_id"],
        "counterparty": signed_contract["counter_party"],
        "collateral": signed_contract["collateral"],
        "event_ids": signed_contract["event_ids"],
        "funding_txid": signed_contract["funding_txid"],
    })
}

/// Converts a failed sign attempt to a JSON value, including error information
/// and the original accepted contract.
fn failed_sign_contract_to_value(failed_sign: &FailedSignContract) -> Value {
    let accepted_contract = accepted_contract_to_value(&failed_sign.accepted_contract);
    json!({
        "state": "failed_signing",
        "contract_id": hex::encode(failed_sign.sign_message.contract_id),
        "accepted_contract": accepted_contract,
        "error_message": failed_sign.error_message,
    })
}

/// Converts a failed accept attempt to a JSON value, including error information
/// and the original offered contract.
fn failed_accept_contract_to_value(failed_accept: &FailedAcceptContract) -> Value {
    let offered_contract = offered_contract_to_value(&failed_accept.offered_contract, "offered");
    json!({
        "state": "failed_accept",
        "offered_contract": offered_contract,
        "error_message": failed_accept.error_message,
    })
}

/// Main conversion function that handles all contract states.
/// Routes to the appropriate conversion function based on the contract's state.
pub fn contract_to_value(contract: &Contract) -> Value {
    match contract {
        Contract::Offered(o) => offered_contract_to_value(o, "offered"),
        Contract::Accepted(a) => accepted_contract_to_value(a),
        Contract::Signed(s) => signed_contract_to_value(s, "signed"),
        Contract::Closed(c) => closed_contract_to_value(c),
        Contract::Refunded(r) => signed_contract_to_value(r, "refunded"),
        Contract::Confirmed(c) => signed_contract_to_value(c, "confirmed"),
        Contract::Rejected(o) => offered_contract_to_value(o, "rejected"),
        Contract::PreClosed(p) => preclosed_contract_to_value(p),
        Contract::FailedSign(f) => failed_sign_contract_to_value(f),
        Contract::FailedAccept(f) => failed_accept_contract_to_value(f),
    }
}

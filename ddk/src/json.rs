use ddk_manager::contract::{
    accepted_contract::AcceptedContract, offered_contract::OfferedContract,
    signed_contract::SignedContract, ClosedContract, Contract, FailedAcceptContract,
    FailedSignContract, PreClosedContract,
};
use dlc_messages::oracle_msgs::EventDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct OracleEvent {
    pub event_id: String,
    pub oracle_pubkey: String,
    pub event_type: String,
}

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

fn failed_sign_contract_to_value(failed_sign: &FailedSignContract) -> Value {
    let accepted_contract = accepted_contract_to_value(&failed_sign.accepted_contract);
    json!({
        "state": "failed_signing",
        "contract_id": hex::encode(failed_sign.sign_message.contract_id),
        "accepted_contract": accepted_contract,
        "error_message": failed_sign.error_message,
    })
}

fn failed_accept_contract_to_value(failed_accept: &FailedAcceptContract) -> Value {
    let offered_contract = offered_contract_to_value(&failed_accept.offered_contract, "offered");
    json!({
        "state": "failed_accept",
        "offered_contract": offered_contract,
        "error_message": failed_accept.error_message,
    })
}

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

use ddk_manager::contract::{
    accepted_contract::AcceptedContract, offered_contract::OfferedContract,
    signed_contract::SignedContract, ClosedContract, Contract, FailedAcceptContract,
    FailedSignContract, PreClosedContract,
};
use serde_json::{json, Value};
use std::collections::HashSet;

pub fn offered_contract_to_string_pretty(
    offered_contracts: Vec<OfferedContract>,
) -> Result<String, serde_json::Error> {
    let offers = offered_contracts
        .iter()
        .map(|offered_contract| {
            let contract_id = hex::encode(offered_contract.id);
            let mut event_ids = HashSet::new();
            for contract_input in &offered_contract.contract_info {
                for announcement in &contract_input.oracle_announcements {
                    let oracle_event = json!({ "event_id": announcement.oracle_event.event_id, "oracle_pubkey": announcement.oracle_public_key.to_string()});
                    event_ids.insert(oracle_event);
                }
            }
            let event_ids = event_ids.into_iter().collect::<Vec<Value>>();
            json!({
                "contract_id": contract_id,
                "is_offer_party": offered_contract.is_offer_party,
                "counterparty": offered_contract.counter_party,
                "collateral": offered_contract.total_collateral,
                "event_ids": event_ids,
            })
        })
        .collect::<Vec<Value>>();
    serde_json::to_string_pretty(&offers)
}

fn offered_contract_to_value(offered_contract: &OfferedContract, state: &str) -> Value {
    let contract_id = hex::encode(offered_contract.id);
    let mut event_ids = HashSet::new();
    for contract_input in &offered_contract.contract_info {
        for announcement in &contract_input.oracle_announcements {
            let oracle_event = json!({ "event_id": announcement.oracle_event.event_id, "oracle_pubkey": announcement.oracle_public_key.to_string()});
            event_ids.insert(oracle_event);
        }
    }
    let event_ids = event_ids.into_iter().collect::<Vec<Value>>();
    json!({
        "state": state,
        "contract_id": contract_id,
        "is_offer_party": offered_contract.is_offer_party,
        "counter_party": offered_contract.counter_party,
        "collateral": offered_contract.total_collateral,
        "event_ids": event_ids,
    })
}

fn accepted_contract_to_value(accepted: &AcceptedContract) -> Value {
    let offered_contract = offered_contract_to_value(&accepted.offered_contract, "offered");
    json!({
        "contract_id": hex::encode(accepted.offered_contract.id),
        "counter_party": offered_contract["counter_party"],
        "collateral": offered_contract["collateral"],
        "event_ids": offered_contract["event_ids"],
        "num_cets": accepted.dlc_transactions.cets.len(),
        "funding_txid": accepted.dlc_transactions.fund.compute_txid(),
        "refund_transaction": accepted.dlc_transactions.refund.compute_txid(),
    })
}

fn signed_contract_to_value(signed: &SignedContract, state: &str) -> Value {
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
        "pnl": closed.pnl,
        "signed_cet": closed.signed_cet,
        "attestations": closed.attestations,
    })
}

fn preclosed_contract_to_value(preclosed: &PreClosedContract) -> Value {
    let signed_contract = signed_contract_to_value(&preclosed.signed_contract, "confirmed");
    json!({
        "state": "preclosed",
        "attestations": preclosed.attestations,
        "signed_cet": preclosed.signed_cet,
        "signed_contract": signed_contract,
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

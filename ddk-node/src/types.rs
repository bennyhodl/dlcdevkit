use ddk::dlc_manager::contract::offered_contract::OfferedContract;
use serde_json::{json, Value};
use std::collections::HashSet;

pub fn offered_contract_to_string_pretty(
    offered_contracts: Vec<OfferedContract>,
) -> Result<String, serde_json::Error> {
    let offers = offered_contracts
        .iter()
        .map(|offered_contract| {
            let contract_id = hex::encode(&offered_contract.id);
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

use std::str::FromStr;

use bitcoin::key::XOnlyPublicKey;
use bitcoin::Amount;
use ddk_dlc::EnumerationPayout;
use ddk_manager::contract::enum_descriptor::EnumDescriptor;
use ddk_manager::contract::{
    contract_input::{ContractInput, ContractInputInfo, OracleInput},
    ContractDescriptor,
};

pub fn create_contract_input(
    outcome_payouts: Vec<EnumerationPayout>,
    offer_collateral: Amount,
    accept_collateral: Amount,
    fee_rate: u64,
    oracle_pubkey: String,
    event_id: String,
) -> ContractInput {
    let contract_descriptor = ContractDescriptor::Enum(EnumDescriptor { outcome_payouts });

    let oracles = OracleInput {
        public_keys: vec![XOnlyPublicKey::from_str(&oracle_pubkey).unwrap()],
        event_id,
        threshold: 1,
    };
    let contract_infos = vec![ContractInputInfo {
        contract_descriptor,
        oracles,
    }];

    ContractInput {
        offer_collateral,
        accept_collateral,
        fee_rate,
        contract_infos,
    }
}

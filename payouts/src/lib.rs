use dlc::EnumerationPayout;
use dlc_manager::contract::{
    contract_input::{ContractInput, ContractInputInfo, OracleInput},
    enum_descriptor::EnumDescriptor,
    ContractDescriptor,
};
use dlc_messages::oracle_msgs::OracleAnnouncement;

pub fn create_contract_input(
    offer_amt: u64,
    accept_amt: u64,
    input: Vec<EnumerationPayout>,
    oracle_ann: OracleAnnouncement,
) -> ContractInput {
    let payouts = EnumDescriptor {
        outcome_payouts: input,
    };
    let contract_info = ContractInputInfo {
        contract_descriptor: ContractDescriptor::Enum(payouts),
        oracles: OracleInput {
            public_keys: vec![oracle_ann.oracle_public_key],
            event_id: oracle_ann.oracle_event.event_id,
            threshold: 1,
        },
    };

    ContractInput {
        offer_collateral: offer_amt,
        accept_collateral: accept_amt,
        fee_rate: 1,
        contract_infos: vec![contract_info],
    }
}

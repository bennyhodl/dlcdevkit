use crate::options_builder::OptionBuilder;
use bitcoin::Amount;
use ddk_manager::{
    contract::{
        contract_input::{ContractInput, ContractInputInfo, OracleInput},
        numerical_descriptor::NumericalDescriptor,
        ContractDescriptor,
    },
    payout_curve::{PayoutFunction, RoundingInterval, RoundingIntervals},
};
use ddk_messages::oracle_msgs::OracleAnnouncement;
use ddk_trie::OracleNumericInfo;

// Helper enums
#[derive(Copy, Clone)]
pub enum OptionType {
    Call,
    Put,
}

#[derive(Copy, Clone)]
pub enum Direction {
    Long,
    Short,
}

fn build_order_offer(
    announcement: &OracleAnnouncement,
    total_collateral: Amount,
    offer_collateral: Amount,
    payout_function: PayoutFunction,
    rounding_intervals: RoundingIntervals,
    fee_rate: u64,
) -> ContractInput {
    let contract_descriptor = ContractDescriptor::Numerical(NumericalDescriptor {
        payout_function,
        rounding_intervals,
        difference_params: None,
        oracle_numeric_infos: OracleNumericInfo {
            nb_digits: vec![20],
            base: 2,
        },
    });

    let oracles = OracleInput {
        public_keys: vec![announcement.oracle_public_key],
        event_id: announcement.oracle_event.event_id.clone(),
        threshold: 1,
    };

    let contract_info = ContractInputInfo {
        oracles,
        contract_descriptor,
    };

    ContractInput {
        contract_infos: vec![contract_info],
        offer_collateral,
        accept_collateral: total_collateral - offer_collateral,
        fee_rate,
    }
}

#[allow(clippy::too_many_arguments)]
// Main option builder function
pub fn build_option_order_offer(
    announcement: &OracleAnnouncement,
    contract_size: Amount,
    strike_price: u64,
    premium: Amount,
    fee_per_byte: u64,
    rounding: u64,
    option_type: OptionType,
    direction: Direction,
    total_collateral: Amount,
    nb_oracle_digits: u32,
) -> anyhow::Result<ContractInput> {
    let payout_function = OptionBuilder::build_option_payout(
        direction,
        option_type,
        strike_price,
        contract_size,
        total_collateral,
        // Oracle Base
        2,
        // Oracle digits
        nb_oracle_digits,
    )?;

    let rounding_mod = compute_rounding_modulus(rounding, contract_size);
    let rounding_intervals =
        create_rounding_intervals(strike_price, rounding_mod, option_type, direction);

    let offer_collateral = match direction {
        Direction::Short => total_collateral - premium,
        Direction::Long => premium,
    };

    Ok(build_order_offer(
        announcement,
        total_collateral,
        offer_collateral,
        payout_function,
        rounding_intervals,
        fee_per_byte,
    ))
}

// Helper function to create rounding intervals
fn create_rounding_intervals(
    strike_price: u64,
    rounding_mod: u64,
    option_type: OptionType,
    direction: Direction,
) -> RoundingIntervals {
    let intervals = match (option_type, direction) {
        (OptionType::Call, Direction::Short) | (OptionType::Call, Direction::Long) => vec![
            RoundingInterval {
                begin_interval: 0,
                rounding_mod: 1,
            },
            RoundingInterval {
                begin_interval: strike_price,
                rounding_mod,
            },
        ],
        (OptionType::Put, Direction::Short) | (OptionType::Put, Direction::Long) => vec![
            RoundingInterval {
                begin_interval: 0,
                rounding_mod,
            },
            RoundingInterval {
                begin_interval: strike_price,
                rounding_mod: 1,
            },
        ],
    };

    RoundingIntervals { intervals }
}

fn compute_rounding_modulus(rounding: u64, total_collateral: Amount) -> u64 {
    (rounding * total_collateral.to_sat()) / 100_000_000
}

// Helper function to create rounding intervals
pub fn create_covered_call_rounding_intervals(
    strike_price: u64,
    rounding_mod: u64,
) -> RoundingIntervals {
    RoundingIntervals {
        intervals: vec![
            // No rounding below strike price
            RoundingInterval {
                begin_interval: 0,
                rounding_mod: 1,
            },
            // Apply rounding above strike price
            RoundingInterval {
                begin_interval: strike_price,
                rounding_mod,
            },
        ],
    }
}

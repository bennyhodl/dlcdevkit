use std::str::FromStr;

use bitcoin::key::XOnlyPublicKey;
use dlc_manager::contract::numerical_descriptor::NumericalDescriptor;
use dlc_manager::{
    contract::{
        contract_input::{ContractInput, ContractInputInfo, OracleInput},
        ContractDescriptor,
    },
    payout_curve::{
        PayoutFunction, PayoutFunctionPiece, PayoutPoint, PolynomialPayoutCurvePiece,
        RoundingInterval, RoundingIntervals,
    },
};
use dlc_trie::OracleNumericInfo;

pub fn generate_payout_curve(
    min_price: u64,
    max_price: u64,
    offer_collateral: u64,
    accept_collateral: u64,
    num_steps: u64,
) -> anyhow::Result<PayoutFunction> {
    let total_collateral = offer_collateral + accept_collateral;
    let price_range = max_price - min_price;
    let step_size = price_range / (num_steps - 1);

    let mut points = Vec::with_capacity(num_steps.try_into().unwrap());

    for i in 0..num_steps {
        let price = if i == num_steps - 1 {
            max_price // Ensure the last point is exactly at max_price
        } else {
            min_price + i * step_size
        };

        let payout = if i == num_steps - 1 {
            total_collateral // Ensure the last payout is the total collateral
        } else {
            (i * total_collateral) / (num_steps - 1)
        };
        points.push(PayoutPoint {
            event_outcome: price,
            extra_precision: 0,
            outcome_payout: payout,
        });
    }

    let payout_curve_pieces = PolynomialPayoutCurvePiece::new(points)?;
    Ok(PayoutFunction::new(vec![
        PayoutFunctionPiece::PolynomialPayoutCurvePiece(payout_curve_pieces),
    ])?)
}

pub fn create_contract_input(
    min_price: u64,
    max_price: u64,
    num_steps: u64,
    offer_collateral: u64,
    accept_collateral: u64,
    fee_rate: u64,
) -> ContractInput {
    let payout_curve = generate_payout_curve(
        min_price,
        max_price,
        offer_collateral,
        accept_collateral,
        num_steps,
    )
    .unwrap();
    let rounding_intervals = RoundingIntervals {
        intervals: vec![RoundingInterval {
            begin_interval: 0,
            rounding_mod: 1,
        }],
    };
    let oracle_numeric_infos = OracleNumericInfo {
        base: 2,
        nb_digits: vec![20],
    };
    let contract_descriptor = ContractDescriptor::Numerical(NumericalDescriptor {
        payout_function: payout_curve,
        rounding_intervals,
        difference_params: None,
        oracle_numeric_infos,
    });

    let oracles = OracleInput {
        public_keys: vec![XOnlyPublicKey::from_str(
            "0d829c1cc556aa59060df5a9543c5357199ace5db9bcd5a8ddd6ee2fc7b6d174",
        )
        .unwrap()],
        event_id: "btcusd1724888198".into(),
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

#[cfg(test)]
mod tests {
    use crate::{create_contract_input, generate_payout_curve};

    #[test]
    fn payout_curve() {
        let curve = generate_payout_curve(13_000, 60_000, 50_000, 50_000, 10);
        assert!(curve.is_ok())
    }

    #[test]
    fn create_contract_input_test() {
        let contract = create_contract_input(0, 100_000, 3, 50_000, 50_000, 2);

        let json = serde_json::to_string(&contract).unwrap();
        println!("{}", json)
    }
}

use crate::options::Direction;
use crate::options::OptionType;
use ddk_manager::payout_curve::HyperbolaPayoutCurvePiece;
use ddk_manager::payout_curve::PayoutFunction;
use ddk_manager::payout_curve::PayoutFunctionPiece;
use ddk_manager::payout_curve::PayoutPoint;
use ddk_manager::payout_curve::PolynomialPayoutCurvePiece;

pub struct OptionBuilder;

impl OptionBuilder {
    // Covered Call (Short Call)
    pub fn build_covered_call_payout(
        strike_price: u64,
        contract_size: u64,
        oracle_base: u32,
        oracle_digits: u32,
    ) -> anyhow::Result<PayoutFunction> {
        let max_outcome = (oracle_base as u64).pow(oracle_digits) - 1;
        let total_collateral = contract_size;

        let below_strike = PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: 0,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
            ])?,
        );

        let above_strike =
            PayoutFunctionPiece::HyperbolaPayoutCurvePiece(HyperbolaPayoutCurvePiece::new(
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: max_outcome,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                true,
                strike_price as f64,
                total_collateral as f64,
                1.0,
                0.0,
                -1.0,
                strike_price as f64,
            )?);

        let payout_function = PayoutFunction::new(vec![below_strike, above_strike])?;
        payout_function.validate(max_outcome)?;

        Ok(payout_function)
    }

    // Short Put
    pub fn build_short_put_payout(
        strike_price: u64,
        total_collateral: u64,
        oracle_base: u32,
        oracle_digits: u32,
    ) -> anyhow::Result<PayoutFunction> {
        let max_outcome = (oracle_base as u64).pow(oracle_digits) - 1;

        let below_strike =
            PayoutFunctionPiece::HyperbolaPayoutCurvePiece(HyperbolaPayoutCurvePiece::new(
                PayoutPoint {
                    event_outcome: 0,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                true,
                0.0,
                0.0,
                1.0,
                0.0,
                1.0,
                strike_price as f64,
            )?);

        let above_strike = PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: max_outcome,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
            ])?,
        );

        let payout_function = PayoutFunction::new(vec![below_strike, above_strike])?;
        payout_function.validate(max_outcome)?;

        Ok(payout_function)
    }

    // Long Call
    pub fn build_long_call_payout(
        strike_price: u64,
        total_collateral: u64,
        oracle_base: u32,
        oracle_digits: u32,
    ) -> anyhow::Result<PayoutFunction> {
        let max_outcome = (oracle_base as u64).pow(oracle_digits) - 1;

        let below_strike = PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: 0,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
            ])?,
        );

        let above_strike =
            PayoutFunctionPiece::HyperbolaPayoutCurvePiece(HyperbolaPayoutCurvePiece::new(
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: max_outcome,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                true,
                strike_price as f64,
                0.0,
                1.0,
                0.0,
                1.0,
                strike_price as f64,
            )?);

        let payout_function = PayoutFunction::new(vec![below_strike, above_strike])?;
        payout_function.validate(max_outcome)?;

        Ok(payout_function)
    }

    // Long Put
    pub fn build_long_put_payout(
        strike_price: u64,
        total_collateral: u64,
        oracle_base: u32,
        oracle_digits: u32,
    ) -> anyhow::Result<PayoutFunction> {
        let max_outcome = (oracle_base as u64).pow(oracle_digits) - 1;

        let below_strike =
            PayoutFunctionPiece::HyperbolaPayoutCurvePiece(HyperbolaPayoutCurvePiece::new(
                PayoutPoint {
                    event_outcome: 0,
                    outcome_payout: total_collateral,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                true,
                0.0,
                total_collateral as f64,
                1.0,
                0.0,
                -1.0,
                strike_price as f64,
            )?);

        let above_strike = PayoutFunctionPiece::PolynomialPayoutCurvePiece(
            PolynomialPayoutCurvePiece::new(vec![
                PayoutPoint {
                    event_outcome: strike_price,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
                PayoutPoint {
                    event_outcome: max_outcome,
                    outcome_payout: 0,
                    extra_precision: 0,
                },
            ])?,
        );

        let payout_function = PayoutFunction::new(vec![below_strike, above_strike])?;
        payout_function.validate(max_outcome)?;

        Ok(payout_function)
    }

    // Main builder function that matches your pattern
    pub fn build_option_payout(
        direction: Direction,
        option_type: OptionType,
        strike_price: u64,
        contract_size: u64,
        total_collateral: u64,
        oracle_base: u32,
        oracle_digits: u32,
    ) -> anyhow::Result<PayoutFunction> {
        match (direction, option_type) {
            (Direction::Short, OptionType::Call) => Self::build_covered_call_payout(
                strike_price,
                contract_size,
                oracle_base,
                oracle_digits,
            ),
            (Direction::Short, OptionType::Put) => Self::build_short_put_payout(
                strike_price,
                total_collateral,
                oracle_base,
                oracle_digits,
            ),
            (Direction::Long, OptionType::Call) => Self::build_long_call_payout(
                strike_price,
                total_collateral,
                oracle_base,
                oracle_digits,
            ),
            (Direction::Long, OptionType::Put) => Self::build_long_put_payout(
                strike_price,
                total_collateral,
                oracle_base,
                oracle_digits,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_option_types() {
        let strike_price = 50000; // $50,000
        let contract_size = 100000000; // 1 BTC in sats
        let oracle_base = 10;
        let oracle_digits = 5;

        // Test covered call
        OptionBuilder::build_option_payout(
            Direction::Short,
            OptionType::Call,
            strike_price,
            contract_size,
            1_000_000,
            oracle_base,
            oracle_digits,
        )
        .unwrap();

        // Test short put
        OptionBuilder::build_option_payout(
            Direction::Short,
            OptionType::Put,
            strike_price,
            contract_size,
            1_000_000,
            oracle_base,
            oracle_digits,
        )
        .unwrap();

        // Test long call
        OptionBuilder::build_option_payout(
            Direction::Long,
            OptionType::Call,
            strike_price,
            contract_size,
            1_000_000,
            oracle_base,
            oracle_digits,
        )
        .unwrap();

        // Test long put
        OptionBuilder::build_option_payout(
            Direction::Long,
            OptionType::Put,
            strike_price,
            contract_size,
            1_000_000,
            oracle_base,
            oracle_digits,
        )
        .unwrap();
    }
}

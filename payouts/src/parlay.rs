use ddk_manager::{
    contract::{
        contract_input::{ContractInput, ContractInputInfo, OracleInput},
        numerical_descriptor::NumericalDescriptor,
        ContractDescriptor,
    },
    payout_curve::{
        PayoutFunction, PayoutFunctionPiece, PayoutPoint, PolynomialPayoutCurvePiece,
        RoundingInterval, RoundingIntervals,
    },
};
use dlc_trie::OracleNumericInfo;

/// Create a complete DLC Contract using oracle-normalized scores
pub fn create_parlay_contract(
    max_normalized_value: u64,
    offer_collateral: u64,
    accept_collateral: u64,
    oracle_input: OracleInput,
    fee_rate: u64,
) -> ContractInput {
    // Create payout function
    let payout_function = create_normalized_payout_function(
        max_normalized_value,
        accept_collateral + offer_collateral,
    );

    // Determine appropriate rounding intervals
    // For efficiency, we might round to nearest 10 for larger values
    let rounding_intervals = RoundingIntervals {
        intervals: vec![
            RoundingInterval {
                begin_interval: 0,
                rounding_mod: 1,
            },
            RoundingInterval {
                begin_interval: 101,
                rounding_mod: 10,
            },
        ],
    };
    // Calculate number of digits needed for the oracle
    let digits_needed = (max_normalized_value as f64).log2().ceil() as u16;

    // Create numerical descriptor
    let numerical_descriptor = NumericalDescriptor {
        payout_function,
        rounding_intervals,
        difference_params: None,
        oracle_numeric_infos: OracleNumericInfo {
            base: 2,
            nb_digits: vec![digits_needed as usize],
        },
    };

    // Create contract descriptor
    let contract_descriptor = ContractDescriptor::Numerical(numerical_descriptor);

    // Create contract info
    let contract_info = ContractInputInfo {
        contract_descriptor,
        oracles: oracle_input,
    };

    // Create final contract input
    ContractInput {
        offer_collateral,
        accept_collateral,
        fee_rate,
        contract_infos: vec![contract_info],
    }
}

/// Create a PayoutFunction for an oracle-normalized score
fn create_normalized_payout_function(
    max_normalized_value: u64, // Typically 1000 or 10000 for 3 or 4 decimal precision
    max_payout: u64,           // Maximum contract payout
) -> PayoutFunction {
    // Create a simple linear polynomial with just two points
    let payout_points = vec![
        PayoutPoint {
            event_outcome: 0,
            outcome_payout: 0,
            extra_precision: 0,
        },
        PayoutPoint {
            event_outcome: max_normalized_value,
            outcome_payout: max_payout,
            extra_precision: 0,
        },
    ];

    // Create a single polynomial piece
    let polynomial_piece = PolynomialPayoutCurvePiece::new(payout_points).unwrap();
    let payout_function_piece = PayoutFunctionPiece::PolynomialPayoutCurvePiece(polynomial_piece);

    // Create the payout function with this single piece
    PayoutFunction::new(vec![payout_function_piece]).unwrap()
}

//! Three independent baseball events sharing one funding output (OR, not AND).
//! Each event has its own 2-of-3 oracle set. Both enums deliberately use the
//! same labels, and every branch has different payouts to expose bad offsets.
//! Oracle 0 matures 60 seconds later; only indexes 1 and 2 attest. Automatic
//! settlement must retain those indexes when filtering by maturity.

use super::*;
use ddk_manager::payout_curve::{PayoutFunctionPiece, PayoutPoint, PolynomialPayoutCurvePiece};

pub const EVENT_IDS: [&str; 3] = ["hitter-two-hits", "hitter-at-bats", "team-wins"];
pub const SIGNERS: [usize; 2] = [1, 2];

pub struct Baseball {
    pub legs: Vec<ContractLeg>,
    pub oracles: Vec<Vec<MemoryOracle>>,
}

impl Baseball {
    pub async fn new(total: Amount, maturity: u32) -> Self {
        let mut legs = Vec::new();
        let mut oracle_sets = Vec::new();
        for (branch, event_id) in EVENT_IDS.iter().enumerate() {
            let oracles = new_oracles(3);
            let (descriptor, announcements) = if branch == 1 {
                let mut announcements = Vec::new();
                for (index, oracle) in oracles.iter().enumerate() {
                    announcements.push(
                        oracle
                            .oracle
                            .create_numeric_event(
                                event_id.to_string(),
                                4,
                                false,
                                0,
                                "at-bats".to_string(),
                                maturity + if index == 0 { 60 } else { 0 },
                            )
                            .await
                            .unwrap(),
                    );
                }
                let points = [(0, false), (4, false), (5, true), (15, true)].map(
                    |(event_outcome, success)| PayoutPoint {
                        event_outcome,
                        outcome_payout: offer_payout(total, branch, success),
                        extra_precision: 0,
                    },
                );
                let pieces = points
                    .windows(2)
                    .map(|pair| {
                        PayoutFunctionPiece::PolynomialPayoutCurvePiece(
                            PolynomialPayoutCurvePiece::new(pair.to_vec()).unwrap(),
                        )
                    })
                    .collect();
                (
                    numeric_descriptor(
                        PayoutFunction::new(pieces).unwrap(),
                        OracleNumericInfo {
                            base: 2,
                            nb_digits: vec![4; 3],
                        },
                        None,
                    ),
                    announcements,
                )
            } else {
                let mut announcements = Vec::new();
                for (index, oracle) in oracles.iter().enumerate() {
                    announcements.push(
                        oracle
                            .oracle
                            .create_enum_event(
                                event_id.to_string(),
                                vec!["yes".into(), "no".into()],
                                maturity + if index == 0 { 60 } else { 0 },
                            )
                            .await
                            .unwrap(),
                    );
                }
                let outcome_payouts = [("yes", true), ("no", false)]
                    .into_iter()
                    .map(|(outcome, success)| {
                        let offer = offer_payout(total, branch, success);
                        EnumerationPayout {
                            outcome: outcome.into(),
                            payout: Payout {
                                offer,
                                accept: total - offer,
                            },
                        }
                    })
                    .collect();
                (
                    ContractDescriptor::Enum(EnumDescriptor { outcome_payouts }),
                    announcements,
                )
            };
            legs.push(ContractLeg::new(descriptor, announcements, 2));
            oracle_sets.push(oracles);
        }
        Self {
            legs,
            oracles: oracle_sets,
        }
    }

    /// Signs only one branch. The other events remain unavailable.
    pub async fn attest(
        &self,
        branch: usize,
        success: bool,
        at_bats: i64,
    ) -> Vec<(usize, OracleAttestation)> {
        for index in SIGNERS {
            let oracle = &self.oracles[branch][index].oracle;
            if branch == 1 {
                oracle
                    .sign_numeric_event(EVENT_IDS[branch].into(), at_bats)
                    .await
                    .unwrap();
            } else {
                oracle
                    .sign_enum_event(
                        EVENT_IDS[branch].into(),
                        if success { "yes" } else { "no" }.into(),
                    )
                    .await
                    .unwrap();
            }
        }
        attestations(&self.oracles[branch], EVENT_IDS[branch], &SIGNERS).await
    }
}

pub fn offer_payout(total: Amount, branch: usize, success: bool) -> Amount {
    let tenths = if success {
        9 - branch as u64
    } else {
        1 + branch as u64
    };
    Amount::from_sat(total.to_sat() / 10 * tenths)
}

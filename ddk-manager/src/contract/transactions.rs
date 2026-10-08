//! The one place a contract's transactions are built.
//!
//! The funding transaction, the CETs and the refund transaction follow from
//! both parties' parameters, the contract infos and the offer's terms. Every
//! path that needs them builds them here: accepting an offer, signing an
//! accept, and the stateless `ddk::contract` module. One builder is what lets
//! the two parties agree on the bytes they sign.

use std::fmt;
use std::ops::Range;

use bitcoin::Amount;
use ddk_dlc::{DlcTransactions, FeeRule, PartyParams};
use ddk_messages::OfferDlc;

use super::contract_info::ContractInfo;
use super::offered_contract::OfferedContract;
use crate::error::Error;

/// The terms of an offer that shape its transactions, apart from the parties
/// and the contract infos.
#[derive(Clone, Debug)]
pub struct ContractTerms {
    /// The sum of both parties' collateral, as the offer declares it.
    pub total_collateral: Amount,
    /// The time after which the refund transaction can be broadcast.
    pub refund_locktime: u32,
    /// The locktime of the CETs.
    pub cet_locktime: u32,
    /// The fee rate of the funding transaction and the CETs, in satoshis per
    /// virtual byte.
    pub fee_rate_per_vb: u64,
    /// The serial id ordering the funding output.
    pub fund_output_serial_id: u64,
    /// The contract feature flags.
    pub contract_flags: u8,
}

impl From<&OfferedContract> for ContractTerms {
    fn from(offered: &OfferedContract) -> Self {
        ContractTerms {
            total_collateral: offered.total_collateral,
            refund_locktime: offered.refund_locktime,
            cet_locktime: offered.cet_locktime,
            fee_rate_per_vb: offered.fee_rate_per_vb,
            fund_output_serial_id: offered.fund_output_serial_id,
            contract_flags: offered.contract_flags,
        }
    }
}

impl From<&OfferDlc> for ContractTerms {
    fn from(offer: &OfferDlc) -> Self {
        ContractTerms {
            total_collateral: offer.get_total_collateral(),
            refund_locktime: offer.refund_locktime,
            cet_locktime: offer.cet_locktime,
            fee_rate_per_vb: offer.fee_rate_per_vb,
            fund_output_serial_id: offer.fund_output_serial_id,
            contract_flags: offer.contract_flags,
        }
    }
}

/// A contract's transactions, with the CETs of each contract info located.
#[derive(Clone)]
pub struct ContractTransactions {
    /// The funding transaction, every contract info's CETs one after another,
    /// and the refund transaction.
    pub transactions: DlcTransactions,
    /// The CETs of each contract info within `transactions.cets`, in offer
    /// order. A contract info's own CET indexes count from the start of its
    /// range.
    pub cet_ranges: Vec<Range<usize>>,
}

/// Why a contract's transactions could not be built.
///
/// The offer and accept variants keep the blame where it belongs so a caller
/// can report an invalid offer or an invalid accept rather than a failed
/// build.
#[derive(Debug)]
pub enum BuildError {
    /// The offer cannot produce a contract on its own: it has no contract info
    /// or no CET.
    Offer(String),
    /// The accepting party's parameters do not fit the offer: the collaterals
    /// do not add up to the offer's total.
    Accept(String),
    /// Payout generation or transaction construction failed.
    Construction(Error),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BuildError::Offer(message) => write!(f, "invalid offer: {message}"),
            BuildError::Accept(message) => write!(f, "invalid accept: {message}"),
            BuildError::Construction(error) => write!(f, "{error}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BuildError::Construction(error) => Some(error),
            BuildError::Offer(_) | BuildError::Accept(_) => None,
        }
    }
}

impl From<Error> for BuildError {
    fn from(error: Error) -> Self {
        BuildError::Construction(error)
    }
}

impl From<ddk_dlc::Error> for BuildError {
    fn from(error: ddk_dlc::Error) -> Self {
        BuildError::Construction(error.into())
    }
}

impl From<BuildError> for Error {
    fn from(error: BuildError) -> Self {
        match error {
            BuildError::Offer(message) | BuildError::Accept(message) => {
                Error::InvalidParameters(message)
            }
            BuildError::Construction(error) => error,
        }
    }
}

/// Builds a contract's transactions from both parties' parameters, the
/// contract infos and the offer's terms.
///
/// The first contract info's payouts give the funding, CET and refund
/// transactions; every further contract info adds its CETs after them. The
/// fee rule is the one the contract was funded under: [`FeeRule::default`]
/// for a new contract, and whichever rule reproduces a signed contract's
/// funding transaction for an existing one.
pub fn build_contract_transactions(
    offer: &PartyParams,
    accept: &PartyParams,
    contract_infos: &[ContractInfo],
    terms: &ContractTerms,
    fee_rule: FeeRule,
) -> Result<ContractTransactions, BuildError> {
    if contract_infos.is_empty() {
        return Err(BuildError::Offer(
            "contract does not contain execution information".to_string(),
        ));
    }
    if offer.collateral + accept.collateral != terms.total_collateral {
        return Err(BuildError::Accept(
            "offer and accept collateral do not equal total collateral".to_string(),
        ));
    }
    let payouts = contract_infos[0].get_payouts(terms.total_collateral)?;
    // A splice input carries a `dlc_input`; when present the funding
    // transaction spends the previous contract's 2-of-2 output and must be
    // built through the spliced constructor.
    let has_dlc_inputs = !offer.dlc_inputs.is_empty() || !accept.dlc_inputs.is_empty();
    let mut transactions = if has_dlc_inputs {
        ddk_dlc::create_spliced_dlc_transactions_with_fee_rule(
            offer,
            accept,
            &payouts,
            terms.refund_locktime,
            terms.fee_rate_per_vb,
            0,
            terms.cet_locktime,
            terms.fund_output_serial_id,
            terms.contract_flags,
            fee_rule,
        )?
    } else {
        ddk_dlc::create_dlc_transactions_with_fee_rule(
            offer,
            accept,
            &payouts,
            terms.refund_locktime,
            terms.fee_rate_per_vb,
            0,
            terms.cet_locktime,
            terms.fund_output_serial_id,
            terms.contract_flags,
            fee_rule,
        )?
    };

    let mut cet_ranges = Vec::with_capacity(contract_infos.len());
    cet_ranges.push(0..transactions.cets.len());
    let cet_input = transactions
        .cets
        .first()
        .ok_or_else(|| BuildError::Offer("contract has no CETs".to_string()))?
        .input[0]
        .clone();
    for contract_info in &contract_infos[1..] {
        let start = transactions.cets.len();
        // The CETs of every contract info after the first have always been
        // built with locktime 0 and a copy of the first CET's input, sequence
        // included. The oracle attestation, not the locktime, gates a CET, and
        // changing either would change the transactions of every stored
        // disjoint contract, so they stay that way.
        transactions.cets.extend(ddk_dlc::create_cets(
            &cet_input,
            &offer.payout_script_pubkey,
            offer.payout_serial_id,
            &accept.payout_script_pubkey,
            accept.payout_serial_id,
            &contract_info.get_payouts(terms.total_collateral)?,
            0,
        ));
        cet_ranges.push(start..transactions.cets.len());
    }

    Ok(ContractTransactions {
        transactions,
        cet_ranges,
    })
}

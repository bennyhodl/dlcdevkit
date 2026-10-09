//! TLV records on an offer that shape the CETs a contract produces.
//!
//! A CET pays the offering party and the accepting party, and the contract
//! descriptor decides how much. A product can need more from an outcome, a
//! payout to a third party for instance, and both parties must build the same
//! CETs for their adaptor signatures to verify, so whatever changes them has
//! to travel on the offer. A CET record is a TLV record on the offer that
//! does. [`PayoutScriptOverrides`] is the first.
//!
//! On the offering side the record goes on `OfferDlc::tlvs` after the offer is
//! created and before it is sent; with the manager, before
//! `Manager::commit_offer`. Both parties then read it from the offer at every
//! build.
//!
//! The library applies every record it knows, whichever party wrote it, and a
//! record can change what the accepting party is paid. The manager rejects an
//! offer carrying a TLV type the application has not allowed
//! (`Manager::with_allowed_offer_tlv_types`); with the stateless module the
//! caller holds the offer. Either way, checking that the records on an offer
//! are ones the accepting party agrees to, before accepting, is the
//! application's job.
//!
//! # Adding a record
//!
//! A record is protocol: a peer that does not apply it builds different CETs
//! and fails signature verification. So the records are built into the
//! library, and adding one means:
//!
//! 1. A record type in `ddk-messages`, with its wire encoding and TLV type.
//!    Pick an odd type, so a peer that does not know it carries it through
//!    rather than rejecting the offer. BOLT 1 starts custom records at 65536;
//!    a type below that should be agreed with the other DLC implementations,
//!    as [`PayoutScriptOverrides`]'s was with node-dlc.
//! 2. An implementation of [`CetRecord`] here.
//! 3. An entry in [`CET_RECORDS`] decoding it, which also fixes where in the
//!    order it applies.
//!
//! A record may change a CET's outputs. It must not change the input, the
//! number of CETs or their order: adaptor signatures and settlement find a CET
//! by its position in its contract info's range. It must be deterministic in
//! what [`CetContext`] holds, because both parties apply it separately and
//! compare nothing but the result. And it must stay within the fee the
//! funding transaction reserved: a CET's fee is priced from both parties'
//! payout scripts when the contract is funded, so a record that puts a larger
//! script or an extra output on a CET underpays the fee rate the parties
//! agreed on.
//!
//! [`CetRecord::validate`] runs when an offer arrives, before any accept
//! exists, so it sees the offer alone; a check that needs the accepting party
//! belongs in [`CetRecord::apply`].

use std::collections::HashSet;
use std::ops::Range;

use bitcoin::{Amount, Transaction, TxOut};
use ddk_dlc::{PartyParams, Payout};
use ddk_messages::{OverrideOutcome, PayoutScriptOverrides, TlvRecord, TlvStream};

use super::contract_info::ContractInfo;
use super::transactions::{BuildError, ContractTerms};
use super::ContractDescriptor;

/// A TLV record on the offer that changes the CETs the contract produces.
///
/// See the [module documentation](self) for what a record may change and the
/// path to adding one.
pub trait CetRecord {
    /// Rejects a record that cannot apply to the offered contract.
    ///
    /// Runs when an offer arrives and before every build, with what the offer
    /// carries: the accepting party is not known yet.
    fn validate(
        &self,
        contract_infos: &[ContractInfo],
        total_collateral: Amount,
    ) -> Result<(), BuildError>;

    /// Rewrites the built CETs. `cets` holds every contract info's CETs one
    /// after another; `contract.cet_ranges` says which are whose.
    fn apply(&self, contract: &CetContext<'_>, cets: &mut [Transaction]) -> Result<(), BuildError>;
}

/// What a record sees when it applies: both parties, the contract infos,
/// where each one's CETs are, and the offer's terms.
pub struct CetContext<'a> {
    /// The offering party's parameters.
    pub offer: &'a PartyParams,
    /// The accepting party's parameters.
    pub accept: &'a PartyParams,
    /// The contract infos, in offer order.
    pub contract_infos: &'a [ContractInfo],
    /// The CETs of each contract info, in the same order.
    pub cet_ranges: &'a [Range<usize>],
    /// The offer's terms.
    pub terms: &'a ContractTerms,
}

/// Decodes one record type from an offer, if the offer carries it.
type CetRecordDecoder = fn(&TlvStream) -> Result<Option<Box<dyn CetRecord>>, BuildError>;

/// The decoders of the record types the library knows, in the order the
/// records apply.
const CET_RECORDS: &[CetRecordDecoder] = &[cet_record::<PayoutScriptOverrides>];

/// The CET records on an offer, in the order they apply.
///
/// A record type appearing more than once on the offer is an error: this
/// crate's stream reads the first copy and node-dlc the last, so two peers
/// would build different CETs from the same offer.
pub fn cet_records(offer_tlvs: &TlvStream) -> Result<Vec<Box<dyn CetRecord>>, BuildError> {
    let mut records = Vec::new();
    for decode in CET_RECORDS {
        records.extend(decode(offer_tlvs)?);
    }
    Ok(records)
}

/// Validates the CET records on an offer against its contract infos, as
/// [`build_contract_transactions`](super::transactions::build_contract_transactions)
/// does before building. Run it when an offer arrives, so a record that
/// cannot apply is rejected with the offer rather than at accept time.
pub fn validate_cet_records(
    contract_infos: &[ContractInfo],
    total_collateral: Amount,
    offer_tlvs: &TlvStream,
) -> Result<(), BuildError> {
    for record in cet_records(offer_tlvs)? {
        record.validate(contract_infos, total_collateral)?;
    }
    Ok(())
}

/// Decodes the record of type `T` on an offer, if there is one.
fn cet_record<T: TlvRecord + CetRecord + 'static>(
    offer_tlvs: &TlvStream,
) -> Result<Option<Box<dyn CetRecord>>, BuildError> {
    let copies = offer_tlvs
        .raw()
        .filter(|record| record.tlv_type == T::TYPE_ID as u64)
        .count();
    if copies > 1 {
        return Err(BuildError::Offer(format!(
            "offer carries {copies} records of type {}, which is one at most",
            T::TYPE_ID
        )));
    }
    let record = offer_tlvs.get::<T>().map_err(|error| {
        BuildError::Offer(format!(
            "record of type {} does not decode: {error:?}",
            T::TYPE_ID
        ))
    })?;
    Ok(record.map(|record| Box::new(record) as Box<dyn CetRecord>))
}

/// How an error names the outcomes of an override.
fn describe(outcome: &OverrideOutcome) -> String {
    match outcome {
        OverrideOutcome::Enum { outcome } => format!("outcome {outcome:?}"),
        OverrideOutcome::Numeric { start, end } => format!("values {start} to {end}"),
    }
}

/// The CETs of `contract_info` that `outcome` covers: each one's position
/// among the contract info's CETs, and its payout. Empty when the contract
/// info is of the other kind or does not have the outcome.
///
/// A numeric CET pays one payout over a range of values. A range that covers
/// some of a CET's values and not the rest leaves that CET's script
/// undecided, so it is an invalid offer.
///
/// A CET's values are the ones it settles. When the oracles use different
/// numbers of digits, the payout curve ends at the largest value the oracle
/// with the fewest digits can attest, and the last CET also settles every
/// larger value the others can, so its range runs to the largest of those.
/// Oracles allowed to disagree by the difference params may attest values on
/// both sides of a CET's range; the CET they settle, and so the script it
/// pays, is the one whose range the trie matched them to.
fn covered_cets(
    contract_info: &ContractInfo,
    outcome: &OverrideOutcome,
    total_collateral: Amount,
) -> Result<Vec<(usize, Payout)>, BuildError> {
    match (&contract_info.contract_descriptor, outcome) {
        (ContractDescriptor::Enum(descriptor), OverrideOutcome::Enum { outcome }) => Ok(descriptor
            .outcome_payouts
            .iter()
            .enumerate()
            .filter(|(_, payout)| payout.outcome == *outcome)
            .map(|(position, payout)| (position, payout.payout.clone()))
            .collect()),
        (ContractDescriptor::Numerical(descriptor), OverrideOutcome::Numeric { start, end }) => {
            let numeric_infos = &descriptor.oracle_numeric_infos;
            let largest_attestable = numeric_infos
                .nb_digits
                .iter()
                .map(|nb_digits| {
                    (numeric_infos.base as u64)
                        .checked_pow(*nb_digits as u32)
                        .map_or(u64::MAX, |values| values - 1)
                })
                .max()
                .unwrap_or(0);
            let ranges = descriptor.get_range_payouts(total_collateral)?;
            let last_position = ranges.len().saturating_sub(1);
            let mut covered = Vec::new();
            for (position, range) in ranges.into_iter().enumerate() {
                let first = range.start as u64;
                let mut last = first + range.count as u64 - 1;
                if position == last_position && numeric_infos.has_diff_nb_digits() {
                    last = last.max(largest_attestable);
                }
                if last < *start || first > *end {
                    continue;
                }
                if first < *start || last > *end {
                    return Err(BuildError::Offer(format!(
                        "payout script override for {} covers part of the CET for values \
                         {first} to {last}",
                        describe(outcome)
                    )));
                }
                covered.push((position, range.payout));
            }
            Ok(covered)
        }
        _ => Ok(Vec::new()),
    }
}

/// For each CET an override covers, in every contract info, the output that
/// would have paid the accepting party's `payout_spk` pays the override script
/// instead.
///
/// The CET is rebuilt with [`ddk_dlc::create_cet`] from the descriptor's
/// payout, the offering party's output and serial ids, the CET's own input
/// and locktime, and an accepting output paying the override script, so the
/// output order and the dust rule are the ones every CET follows. The
/// accepting party's output is never looked up by script, which both parties
/// may share.
impl CetRecord for PayoutScriptOverrides {
    fn validate(
        &self,
        contract_infos: &[ContractInfo],
        total_collateral: Amount,
    ) -> Result<(), BuildError> {
        // Each CET an override covers, by contract info and position, so two
        // overrides cannot give one CET two scripts.
        let mut covered = HashSet::new();
        for override_ in &self.overrides {
            let outcome = describe(&override_.outcome);
            if override_.script_pubkey.is_empty() {
                return Err(BuildError::Offer(format!(
                    "payout script override for {outcome} has an empty script"
                )));
            }
            if let OverrideOutcome::Numeric { start, end } = override_.outcome {
                if start > end {
                    return Err(BuildError::Offer(format!(
                        "payout script override for {outcome} has its start after its end"
                    )));
                }
            }
            let mut covers_any = false;
            for (index, contract_info) in contract_infos.iter().enumerate() {
                for (position, _) in
                    covered_cets(contract_info, &override_.outcome, total_collateral)?
                {
                    covers_any = true;
                    if !covered.insert((index, position)) {
                        return Err(BuildError::Offer(format!(
                            "payout script override for {outcome} covers a CET another \
                             override covers"
                        )));
                    }
                }
            }
            if !covers_any {
                return Err(BuildError::Offer(format!(
                    "payout script override for {outcome} covers no CET of the offer"
                )));
            }
        }
        Ok(())
    }

    fn apply(&self, contract: &CetContext<'_>, cets: &mut [Transaction]) -> Result<(), BuildError> {
        for override_ in &self.overrides {
            // Funding priced the CET fee from the accepting party's payout
            // script; a longer script in its place underpays the fee rate.
            let accept_script_len = contract.accept.payout_script_pubkey.len();
            if override_.script_pubkey.len() > accept_script_len {
                return Err(BuildError::Accept(format!(
                    "payout script override for {} is {} bytes, longer than the accepting \
                     party's {accept_script_len}-byte payout script the CET fee was reserved for",
                    describe(&override_.outcome),
                    override_.script_pubkey.len()
                )));
            }
            for (contract_info, cet_range) in
                contract.contract_infos.iter().zip(contract.cet_ranges)
            {
                for (position, payout) in covered_cets(
                    contract_info,
                    &override_.outcome,
                    contract.terms.total_collateral,
                )? {
                    let cet = &mut cets[cet_range.start + position];
                    *cet = ddk_dlc::create_cet(
                        TxOut {
                            value: payout.offer,
                            script_pubkey: contract.offer.payout_script_pubkey.clone(),
                        },
                        contract.offer.payout_serial_id,
                        TxOut {
                            value: payout.accept,
                            script_pubkey: override_.script_pubkey.clone(),
                        },
                        contract.accept.payout_serial_id,
                        &cet.input[0],
                        cet.lock_time.to_consensus_u32(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::numerical_descriptor::NumericalDescriptor;
    use crate::payout_curve::{
        PayoutFunction, PayoutFunctionPiece, PayoutPoint, PolynomialPayoutCurvePiece,
        RoundingInterval, RoundingIntervals,
    };
    use ddk_messages::PayoutScriptOverride;
    use ddk_trie::OracleNumericInfo;

    const TOTAL_COLLATERAL: Amount = Amount::from_sat(300);

    /// A numeric contract info whose curve pays 0, 100, 200 and 300 sats to
    /// the offering party at the values 0 to 3, one CET each, attested by
    /// oracles with `nb_digits` binary digits, any two of which settle it.
    fn numeric_contract_info(nb_digits: Vec<usize>) -> ContractInfo {
        let point = |event_outcome, outcome_payout| PayoutPoint {
            event_outcome,
            outcome_payout,
            extra_precision: 0,
        };
        let payout_function =
            PayoutFunction::new(vec![PayoutFunctionPiece::PolynomialPayoutCurvePiece(
                PolynomialPayoutCurvePiece::new(vec![
                    point(0, Amount::ZERO),
                    point(3, TOTAL_COLLATERAL),
                ])
                .unwrap(),
            )])
            .unwrap();
        ContractInfo {
            contract_descriptor: ContractDescriptor::Numerical(NumericalDescriptor {
                payout_function,
                rounding_intervals: RoundingIntervals {
                    intervals: vec![RoundingInterval {
                        begin_interval: 0,
                        rounding_mod: 1,
                    }],
                },
                difference_params: None,
                oracle_numeric_infos: OracleNumericInfo { base: 2, nb_digits },
            }),
            oracle_announcements: vec![],
            threshold: 2,
        }
    }

    fn overriding(start: u64, end: u64) -> PayoutScriptOverrides {
        PayoutScriptOverrides {
            overrides: vec![PayoutScriptOverride {
                outcome: OverrideOutcome::Numeric { start, end },
                script_pubkey: bitcoin::ScriptBuf::from_bytes(vec![0x00, 0x14, 0xaa]),
            }],
        }
    }

    /// With oracles of two and four digits, the last CET settles 3 and every
    /// value up to 15 the four-digit oracles can attest, so a range has to
    /// cover all of them to override it.
    #[test]
    fn a_numeric_override_covers_the_values_the_last_cet_settles_above_the_curve() {
        let contract_infos = [numeric_contract_info(vec![2, 4, 4])];
        let validate =
            |start, end| overriding(start, end).validate(&contract_infos, TOTAL_COLLATERAL);
        assert!(validate(3, 3).is_err());
        assert!(validate(3, 14).is_err());
        assert!(validate(3, 15).is_ok());
        assert!(validate(3, u64::MAX).is_ok());
        assert!(validate(2, 2).is_ok());
    }

    /// With oracles of the same number of digits, the curve's last value is the
    /// largest any of them can attest, and the last CET settles it alone.
    #[test]
    fn a_numeric_override_of_the_last_value_covers_its_cet_when_digits_agree() {
        let contract_infos = [numeric_contract_info(vec![2, 2, 2])];
        assert!(overriding(3, 3)
            .validate(&contract_infos, TOTAL_COLLATERAL)
            .is_ok());
    }
}

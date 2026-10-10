//! Offer creation and validation.

use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_messages::OfferDlc;

use super::context::{ensure_protocol_version, execution_infos, validate_offer_funding_inputs};
use super::error::ContractError;
use super::types::{random_serial_id, random_temporary_contract_id, CreateOfferParams};
use super::PROTOCOL_VERSION;

/// Creates an offer message from explicit contract and Bitcoin data.
///
/// No secret key is required: the offer carries the offering party's DLC
/// funding *public* key, and funding inputs are signed later through the PSBT
/// signing layer. Serial ids and the temporary contract id are randomly
/// generated when omitted from `params`. The built offer must pass
/// [`validate_offer_structure`] apart from the oracle announcement check, so a
/// malformed offer never leaves this party.
pub fn create_offer(params: CreateOfferParams) -> Result<OfferDlc, ContractError> {
    let CreateOfferParams {
        chain_hash,
        temporary_contract_id,
        contract_info,
        offer_collateral,
        party,
        fund_output_serial_id,
        fee_rate_per_vb,
        cet_locktime,
        refund_locktime,
        contract_flags,
    } = params;

    let cet_locktime = cet_locktime.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the unix epoch")
            .as_secs() as u32
    });

    let offer = OfferDlc {
        protocol_version: PROTOCOL_VERSION,
        contract_flags,
        chain_hash,
        temporary_contract_id: temporary_contract_id.unwrap_or_else(random_temporary_contract_id),
        contract_info,
        funding_pubkey: party.funding_pubkey,
        payout_spk: party.payout_spk,
        payout_serial_id: party.payout_serial_id.unwrap_or_else(random_serial_id),
        offer_collateral,
        funding_inputs: party.funding_inputs,
        change_spk: party.change_spk,
        change_serial_id: party.change_serial_id.unwrap_or_else(random_serial_id),
        fund_output_serial_id: fund_output_serial_id.unwrap_or_else(random_serial_id),
        fee_rate_per_vb,
        cet_locktime,
        refund_locktime,
        tlvs: Default::default(),
    };
    // The offering party chose its own oracles, so it skips the announcement
    // check; verifying them is the receiving party's job.
    validate_offer_terms(&offer)?;
    Ok(offer)
}

/// Validates an offer's structure: the checks that do not depend on the
/// receiver's timeout policy or clock.
///
/// Checks the oracle announcement signatures and events, the protocol
/// version, the fee rate, the collateral against the total and the dust
/// limit, that the payout and change scripts are standard, that the CET and
/// refund locktimes share a unit and are ordered (with the CET locktime no
/// later than the closest maturity), that the funding input serial ids are
/// unique and the change and fund output serial ids differ, that any splice
/// input spends the 2-of-2 it names, and that the payouts cover every outcome
/// the oracles can attest.
///
/// It does not check ordinary funding inputs: their previous transactions are
/// not decoded, so neither the input values nor the specification's SegWit
/// requirement is verified here.
///
/// A party that stores or relays offers without accepting them has no timeout
/// policy to apply. [`validate_offer`] adds the policy.
pub fn validate_offer_structure(offer: &OfferDlc) -> Result<(), ContractError> {
    offer
        .validate_announcements(&Secp256k1::verification_only())
        .map_err(|e| ContractError::InvalidOffer(e.to_string()))?;
    validate_offer_terms(offer)
}

/// The structural checks shared by offer creation and validation: all of
/// [`validate_offer_structure`] except the oracle announcements.
fn validate_offer_terms(offer: &OfferDlc) -> Result<(), ContractError> {
    ensure_protocol_version(offer.protocol_version, ContractError::InvalidOffer)?;
    offer
        .validate_terms()
        .map_err(|e| ContractError::InvalidOffer(e.to_string()))?;
    validate_offer_funding_inputs(&offer.funding_inputs)?;
    execution_infos(offer)?;
    Ok(())
}

/// Validates an incoming offer's structure and timeout policy.
///
/// Runs [`validate_offer_structure`], then the accepting party's policy.
/// `min_timeout_interval` and `max_timeout_interval` bound the distance between
/// the oracle event maturity and the offer's refund locktime. `now_unix` is the
/// accepting party's clock; an offer whose closest oracle event has already
/// matured is rejected.
pub fn validate_offer(
    offer: &OfferDlc,
    min_timeout_interval: u32,
    max_timeout_interval: u32,
    now_unix: u64,
) -> Result<(), ContractError> {
    validate_offer_structure(offer)?;
    offer
        .validate_timing(min_timeout_interval, max_timeout_interval, now_unix)
        .map_err(|e| ContractError::InvalidOffer(e.to_string()))
}

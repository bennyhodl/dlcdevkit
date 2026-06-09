//! Offer creation and validation.

use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_messages::OfferDlc;

use super::context::{ensure_no_dlc_inputs, ensure_protocol_version};
use super::error::ContractError;
use super::types::{random_serial_id, random_temporary_contract_id, CreateOfferParams};
use super::PROTOCOL_VERSION;

/// Creates an offer message from explicit contract and Bitcoin data.
///
/// No secret key is required: the offer carries the offering party's DLC
/// funding *public* key, and funding inputs are signed later through the PSBT
/// signing layer. Serial ids and the temporary contract id are randomly
/// generated when omitted from `params`.
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

    ensure_no_dlc_inputs(&party.funding_inputs)?;
    ddk_dlc::util::validate_fee_rate(fee_rate_per_vb)
        .map_err(|e| ContractError::InvalidOffer(format!("invalid fee rate: {e}")))?;
    if cet_locktime >= refund_locktime {
        return Err(ContractError::InvalidOffer(
            "refund locktime must be after the CET locktime".to_string(),
        ));
    }
    let mut input_serial_ids = party
        .funding_inputs
        .iter()
        .map(|input| input.input_serial_id)
        .collect::<Vec<_>>();
    input_serial_ids.sort_unstable();
    if input_serial_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ContractError::InvalidFundingInput(
            "funding input serial ids are not unique".to_string(),
        ));
    }

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
    };

    if offer.offer_collateral > offer.get_total_collateral() {
        return Err(ContractError::InvalidOffer(
            "offer collateral exceeds total collateral".to_string(),
        ));
    }
    // Catch malformed payout or oracle data before the offer leaves this party.
    let execution_infos = ddk_manager::contract::execution_contract_infos(&offer.contract_info)?;
    if execution_infos.is_empty() {
        return Err(ContractError::InvalidOffer(
            "contract does not contain execution information".to_string(),
        ));
    }
    for info in &execution_infos {
        info.validate()?;
    }

    Ok(offer)
}

/// Validates an incoming offer's structure, oracle announcements, and timeout policy.
///
/// `min_timeout_interval` and `max_timeout_interval` bound the distance between
/// the oracle event maturity and the offer's refund locktime, and are the
/// accepting party's local policy.
pub fn validate_offer(
    offer: &OfferDlc,
    min_timeout_interval: u32,
    max_timeout_interval: u32,
) -> Result<(), ContractError> {
    ensure_protocol_version(offer.protocol_version, ContractError::InvalidOffer)?;
    ensure_no_dlc_inputs(&offer.funding_inputs)?;
    ddk_dlc::util::validate_fee_rate(offer.fee_rate_per_vb)
        .map_err(|e| ContractError::InvalidOffer(format!("invalid fee rate: {e}")))?;
    if offer.offer_collateral > offer.get_total_collateral() {
        return Err(ContractError::InvalidOffer(
            "offer collateral exceeds total collateral".to_string(),
        ));
    }
    offer
        .validate(
            &Secp256k1::verification_only(),
            min_timeout_interval,
            max_timeout_interval,
        )
        .map_err(|e| ContractError::InvalidOffer(e.to_string()))?;
    Ok(())
}

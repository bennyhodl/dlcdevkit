//! Offer creation and validation.

use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_manager::contract::cet_records::validate_cet_records;
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
/// [`validate_offer_structure`], so this party never sends an offer its
/// counterparty would reject.
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
    validate_offer_structure(&offer)?;
    Ok(offer)
}

/// Validates an offer's structure: the checks that do not depend on the
/// receiver's timeout policy or clock.
///
/// Checks the oracle announcement signatures and events (an enum event
/// commits to exactly one nonce, so an oracle that commits one per outcome is
/// rejected), the protocol version, the fee rate, the collateral against the
/// total and the dust limit, that the payout and change scripts are standard,
/// that the CET and refund locktimes share a unit and are ordered (with the
/// CET locktime no later than the closest maturity), that the funding input
/// serial ids are unique and the change and fund output serial ids differ,
/// that any splice input spends the 2-of-2 it names, that the payouts cover
/// every outcome the oracles can attest, and that every CET record on the
/// offer applies to the contract.
///
/// Zero offer collateral and a zero CET locktime are allowed. The first is a
/// contract the accepting party funds alone, the second one whose CETs are not
/// time-locked and settle as soon as the oracles attest. Both build valid
/// transactions; an application that will not take part in them refuses them
/// as its own policy.
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
    ensure_protocol_version(offer.protocol_version, ContractError::InvalidOffer)?;
    offer
        .validate_terms()
        .map_err(|e| ContractError::InvalidOffer(e.to_string()))?;
    validate_offer_funding_inputs(&offer.funding_inputs)?;
    // A CET record the contract cannot carry is as much a reason to reject the
    // offer as a malformed descriptor, and is checked before any accept exists.
    validate_cet_records(
        &execution_infos(offer)?,
        offer.get_total_collateral(),
        &offer.tlvs,
    )?;
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

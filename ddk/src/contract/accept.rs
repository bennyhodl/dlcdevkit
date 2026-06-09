//! Offer acceptance and transaction reconstruction.

use ddk_dlc::secp256k1_zkp::{PublicKey, Secp256k1, SecretKey};
use ddk_dlc::DlcTransactions;
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, OfferDlc};

use super::context::{
    build_context, context_from_messages, create_adaptor_signatures, create_refund_signature,
    dlc_party_params, ensure_no_dlc_inputs, ensure_unique_input_serial_ids,
};
use super::create::validate_offer;
use super::error::ContractError;
use super::psbt::build_funding_psbt;
use super::types::{random_serial_id, AcceptOfferParams, AcceptResult};

/// Validates an offer and creates the accepting party's wire message.
///
/// The accept collateral is the offer's total collateral minus the offer
/// collateral. The returned [`AcceptResult`] carries the accept message to
/// send back, the rebuilt contract transactions, and a funding PSBT ready for
/// the PSBT signing layer. Serial ids are randomly generated when omitted.
///
/// `funding_secret_key` is the accepting party's DLC funding key, used here to
/// produce CET adaptor signatures and the refund signature. It must match
/// `params.party.funding_pubkey`.
pub fn accept_offer(
    offer: &OfferDlc,
    params: AcceptOfferParams,
    funding_secret_key: &SecretKey,
) -> Result<AcceptResult, ContractError> {
    let AcceptOfferParams {
        party,
        min_timeout_interval,
        max_timeout_interval,
    } = params;

    validate_offer(offer, min_timeout_interval, max_timeout_interval)?;
    ensure_no_dlc_inputs(&party.funding_inputs)?;

    let secp = Secp256k1::new();
    if PublicKey::from_secret_key(&secp, funding_secret_key) != party.funding_pubkey {
        return Err(ContractError::InvalidAccept(
            "funding secret key does not match the accept party funding public key".to_string(),
        ));
    }
    let accept_collateral = offer
        .get_total_collateral()
        .checked_sub(offer.offer_collateral)
        .ok_or_else(|| {
            ContractError::InvalidOffer("offer collateral exceeds total collateral".to_string())
        })?;

    let payout_serial_id = party.payout_serial_id.unwrap_or_else(random_serial_id);
    let change_serial_id = party.change_serial_id.unwrap_or_else(random_serial_id);
    let accept_params = dlc_party_params(
        party.funding_pubkey,
        party.payout_spk.clone(),
        payout_serial_id,
        party.change_spk.clone(),
        change_serial_id,
        accept_collateral,
        &party.funding_inputs,
    )?;
    let context = build_context(offer, &accept_params)?;
    let adaptor_signatures = create_adaptor_signatures(
        &secp,
        &context,
        funding_secret_key,
        offer.get_total_collateral(),
    )?;
    let refund_signature = create_refund_signature(&secp, &context, funding_secret_key)?;

    let accept = AcceptDlc {
        protocol_version: offer.protocol_version,
        temporary_contract_id: offer.temporary_contract_id,
        accept_collateral,
        funding_pubkey: party.funding_pubkey,
        payout_spk: party.payout_spk,
        payout_serial_id,
        funding_inputs: party.funding_inputs,
        change_spk: party.change_spk,
        change_serial_id,
        cet_adaptor_signatures: CetAdaptorSignatures::from(adaptor_signatures.as_slice()),
        refund_signature,
        negotiation_fields: None,
    };
    ensure_unique_input_serial_ids(offer, &accept)?;

    let funding_psbt = build_funding_psbt(offer, &accept, context.transactions.fund.clone())?;
    Ok(AcceptResult {
        accept,
        transactions: context.transactions,
        funding_psbt,
    })
}

/// Rebuilds the unsigned funding, CET, and refund transactions from wire messages.
///
/// The result is deterministic: both parties rebuild identical transactions
/// from the same offer and accept messages, so neither has to trust
/// transaction data supplied by the other.
pub fn create_dlc_transactions(
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> Result<DlcTransactions, ContractError> {
    Ok(context_from_messages(offer, accept)?.transactions)
}

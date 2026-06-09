//! Funding transaction completion by the accepting party.

use bitcoin::psbt::Psbt;
use bitcoin::Transaction;
use ddk_dlc::secp256k1_zkp::Secp256k1;
use ddk_messages::{AcceptDlc, FundingSignatures, OfferDlc, SignDlc};

use super::context::{
    apply_funding_signatures, context_from_messages, contract_id_from_transactions,
    ensure_protocol_version, verify_counterparty_signatures, ContractContext,
};
use super::error::ContractError;
use super::psbt::{ensure_psbt_matches_funding_transaction, extract_funding_signatures};
use super::types::Party;

/// Verifies the sign message and completes the funding transaction.
///
/// `signed_funding_psbt` must contain finalized witnesses for every
/// accept-side funding input; for single-funded contracts with no accept-side
/// inputs the unsigned funding PSBT is sufficient. The returned transaction is
/// fully signed and ready to broadcast through the caller's blockchain client
/// (for example [`ddk_manager::Blockchain::send_transaction`]); this function
/// performs no network access.
pub fn finalize_sign(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    signed_funding_psbt: &Psbt,
) -> Result<Transaction, ContractError> {
    let context = context_from_messages(offer, accept)?;
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &context.transactions.fund)?;
    let funding_signatures =
        extract_funding_signatures(offer, accept, Party::Accept, signed_funding_psbt)?;
    finalize_with_context(offer, accept, sign, funding_signatures, context)
}

/// Completes the funding transaction from already extracted accept-side witnesses.
pub(crate) fn finalize_sign_internal(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_signatures: FundingSignatures,
) -> Result<Transaction, ContractError> {
    let context = context_from_messages(offer, accept)?;
    finalize_with_context(offer, accept, sign, funding_signatures, context)
}

fn finalize_with_context(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_signatures: FundingSignatures,
    context: ContractContext,
) -> Result<Transaction, ContractError> {
    if funding_signatures.funding_signatures.len() != accept.funding_inputs.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} accept funding signatures, received {}",
            accept.funding_inputs.len(),
            funding_signatures.funding_signatures.len()
        )));
    }
    ensure_protocol_version(sign.protocol_version, ContractError::InvalidSign)?;
    if sign.protocol_version != offer.protocol_version {
        return Err(ContractError::InvalidSign(
            "offer and sign protocol versions differ".to_string(),
        ));
    }
    if sign.funding_signatures.funding_signatures.len() != offer.funding_inputs.len() {
        return Err(ContractError::InvalidSign(format!(
            "sign message carries {} funding signatures but the offer has {} funding inputs",
            sign.funding_signatures.funding_signatures.len(),
            offer.funding_inputs.len()
        )));
    }

    let expected_contract_id =
        contract_id_from_transactions(&context.transactions, &offer.temporary_contract_id);
    if sign.contract_id != expected_contract_id {
        return Err(ContractError::InvalidSign(
            "sign message contract id does not match the rebuilt funding transaction".to_string(),
        ));
    }
    let secp = Secp256k1::new();
    verify_counterparty_signatures(
        &secp,
        &context,
        offer.get_total_collateral(),
        offer.funding_pubkey,
        &sign.refund_signature,
        &sign.cet_adaptor_signatures,
        ContractError::InvalidSign,
    )?;

    let mut funding_transaction = context.transactions.fund;
    apply_funding_signatures(
        &mut funding_transaction,
        offer,
        accept,
        Party::Offer,
        &sign.funding_signatures,
    )?;
    apply_funding_signatures(
        &mut funding_transaction,
        offer,
        accept,
        Party::Accept,
        &funding_signatures,
    )?;

    Ok(funding_transaction)
}

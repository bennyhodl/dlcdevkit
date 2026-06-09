//! Sign message creation by the offering party.

use bitcoin::psbt::Psbt;
use ddk_dlc::secp256k1_zkp::{Secp256k1, SecretKey};
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, FundingSignatures, OfferDlc, SignDlc};

use super::context::{
    context_from_messages, contract_id_from_transactions, create_adaptor_signatures,
    create_refund_signature, ensure_funding_key, verify_counterparty_signatures, ContractContext,
};
use super::error::ContractError;
use super::psbt::{ensure_psbt_matches_funding_transaction, extract_funding_signatures};
use super::types::{Party, SignResult};

/// Verifies the accept message and creates the offering party's sign message.
///
/// `signed_funding_psbt` must contain finalized witnesses for every offer-side
/// funding input; how they got there (wallet, xpriv, descriptor, or an
/// external signer) does not matter. The PSBT is verified against the funding
/// transaction rebuilt from the messages before any signature is extracted.
///
/// `funding_secret_key` is the offering party's DLC funding key, used to
/// produce CET adaptor signatures and the refund signature.
pub fn sign_accept(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    signed_funding_psbt: &Psbt,
) -> Result<SignResult, ContractError> {
    let context = context_from_messages(offer, accept)?;
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &context.transactions.fund)?;
    let funding_signatures =
        extract_funding_signatures(offer, accept, Party::Offer, signed_funding_psbt)?;
    sign_with_context(
        offer,
        accept,
        funding_secret_key,
        funding_signatures,
        context,
    )
}

/// Creates the sign message from already extracted offer-side funding witnesses.
pub(crate) fn sign_accept_internal(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    funding_signatures: FundingSignatures,
) -> Result<SignResult, ContractError> {
    let context = context_from_messages(offer, accept)?;
    sign_with_context(
        offer,
        accept,
        funding_secret_key,
        funding_signatures,
        context,
    )
}

fn sign_with_context(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    funding_signatures: FundingSignatures,
    context: ContractContext,
) -> Result<SignResult, ContractError> {
    if funding_signatures.funding_signatures.len() != offer.funding_inputs.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} offer funding signatures, received {}",
            offer.funding_inputs.len(),
            funding_signatures.funding_signatures.len()
        )));
    }
    let secp = Secp256k1::new();
    ensure_funding_key(
        &secp,
        funding_secret_key,
        &offer.funding_pubkey,
        ContractError::InvalidOffer,
    )?;
    verify_counterparty_signatures(
        &secp,
        &context,
        offer.get_total_collateral(),
        accept.funding_pubkey,
        &accept.refund_signature,
        &accept.cet_adaptor_signatures,
        ContractError::InvalidAccept,
    )?;
    let adaptor_signatures = create_adaptor_signatures(
        &secp,
        &context,
        funding_secret_key,
        offer.get_total_collateral(),
    )?;
    let refund_signature = create_refund_signature(&secp, &context, funding_secret_key)?;

    let sign = SignDlc {
        protocol_version: offer.protocol_version,
        contract_id: contract_id_from_transactions(
            &context.transactions,
            &offer.temporary_contract_id,
        ),
        cet_adaptor_signatures: CetAdaptorSignatures::from(adaptor_signatures.as_slice()),
        refund_signature,
        funding_signatures,
    };
    Ok(SignResult {
        sign,
        transactions: context.transactions,
    })
}

//! Offer acceptance and transaction reconstruction.

use ddk_dlc::secp256k1_zkp::SecretKey;
use ddk_dlc::DlcTransactions;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};

use super::context::{context_from_messages, signed_context};
use super::error::ContractError;
use super::external::{accept_request, complete_accept, sign_contract};
use super::types::{AcceptOfferParams, AcceptResult};

/// Validates an offer and creates the accepting party's wire message.
///
/// The accept collateral is the offer's total collateral minus the offer
/// collateral. The returned [`AcceptResult`] carries the accept message to
/// send back, the rebuilt contract transactions, and a funding PSBT ready for
/// the PSBT signing layer. Serial ids are randomly generated when omitted.
///
/// `funding_secret_key` is the accepting party's DLC funding key, used here to
/// produce CET adaptor signatures and the refund signature. It must match
/// `params.party.funding_pubkey`. A signer that keeps the key outside the
/// process goes through [`external::accept_request`](super::external::accept_request)
/// instead; this is that request answered in the process.
pub fn accept_offer(
    offer: &OfferDlc,
    params: AcceptOfferParams,
    funding_secret_key: &SecretKey,
) -> Result<AcceptResult, ContractError> {
    let request = accept_request(offer, params)?;
    let signatures = sign_contract(
        request.contract(),
        funding_secret_key,
        ContractError::InvalidAccept,
    )?;
    complete_accept(request, signatures)
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

/// Rebuilds a signed contract's transactions under the [`ddk_dlc::FeeRule`]
/// whose funding transaction matches `sign`'s contract id.
///
/// Tries [`ddk_dlc::FeeRule::CounterpartyPayout`] first, then
/// [`ddk_dlc::FeeRule::OwnPayoutOnly`]. Fails when neither rule matches.
pub fn create_signed_dlc_transactions(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
) -> Result<DlcTransactions, ContractError> {
    Ok(signed_context(offer, accept, sign)?.transactions)
}

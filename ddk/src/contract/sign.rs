//! Sign message creation by the offering party.

use bitcoin::psbt::Psbt;
use ddk_dlc::secp256k1_zkp::SecretKey;
use ddk_messages::{AcceptDlc, OfferDlc};

use super::error::ContractError;
use super::external::{complete_sign, sign_contract, sign_request, sign_splice_inputs};
use super::types::{DlcInputSigningKey, SignResult};

/// Verifies the accept message and creates the offering party's sign message.
///
/// `signed_funding_psbt` must contain finalized witnesses for every offer-side
/// wallet input; how they got there (wallet, xpriv, descriptor, or an external
/// signer) does not matter. A splice input needs this party's half of its
/// 2-of-2 as a partial signature in the PSBT, as an external signer leaves it;
/// to sign it here with the previous contract's key, use
/// [`sign_accept_spliced`]. The PSBT is verified against the funding
/// transaction rebuilt from the messages before any signature is extracted.
///
/// `funding_secret_key` is the offering party's DLC funding key, used to
/// produce CET adaptor signatures and the refund signature. A signer that
/// keeps it outside the process goes through
/// [`external::sign_request`](super::external::sign_request) instead; this is
/// that request answered in the process.
pub fn sign_accept(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    signed_funding_psbt: &Psbt,
) -> Result<SignResult, ContractError> {
    sign_accept_spliced(offer, accept, funding_secret_key, signed_funding_psbt, &[])
}

/// Verifies the accept message and creates the offering party's sign message,
/// signing this party's half of each splice (DLC) funding input.
///
/// Behaves like [`sign_accept`] for ordinary funding inputs. For each DLC
/// (splice) input in the offer, `dlc_input_keys` supplies the previous
/// contract's funding secret key (matched by serial id); this party produces
/// its half of the prior 2-of-2 signature, which the accepting party verifies
/// and completes in [`finalize_sign_spliced`](super::finalize_sign_spliced).
/// A splice input without a key must already carry the half in
/// `signed_funding_psbt`.
pub fn sign_accept_spliced(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    signed_funding_psbt: &Psbt,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<SignResult, ContractError> {
    let request = sign_request(offer, accept)?;
    let signatures = sign_contract(
        request.contract(),
        funding_secret_key,
        ContractError::InvalidOffer,
    )?;
    let mut signed_funding_psbt = signed_funding_psbt.clone();
    sign_splice_inputs(request.funding(), dlc_input_keys, &mut signed_funding_psbt)?;
    complete_sign(request, signatures, &signed_funding_psbt)
}

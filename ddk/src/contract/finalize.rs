//! Funding transaction completion by the accepting party.

use bitcoin::psbt::Psbt;
use bitcoin::Transaction;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};

use super::error::ContractError;
use super::external::{complete_finalize, finalize_request, sign_splice_inputs};
use super::types::DlcInputSigningKey;

/// Verifies the sign message and completes the funding transaction.
///
/// `signed_funding_psbt` must contain finalized witnesses for every
/// accept-side funding input; for single-funded contracts with no accept-side
/// inputs the unsigned funding PSBT is sufficient. A splice input needs this
/// party's half of its 2-of-2 as a partial signature in the PSBT, as an
/// external signer leaves it; to sign it here with the previous contract's
/// key, use [`finalize_sign_spliced`]. The returned transaction is fully
/// signed and ready to broadcast through the caller's blockchain client (for
/// example [`ddk_manager::Blockchain::send_transaction`]); this function
/// performs no network access.
pub fn finalize_sign(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    signed_funding_psbt: &Psbt,
) -> Result<Transaction, ContractError> {
    finalize_sign_spliced(offer, accept, sign, signed_funding_psbt, &[])
}

/// Verifies the sign message and completes the funding transaction, signing
/// this party's half of each splice (DLC) funding input.
///
/// Behaves like [`finalize_sign`] for ordinary funding inputs. For each DLC
/// (splice) input in the offer, `dlc_input_keys` supplies this (accepting)
/// party's previous contract funding secret key (matched by serial id). The
/// offering party's half signature is verified before this party's half is
/// produced and the two are combined into the input's final 2-of-2 witness. A
/// splice input without a key must already carry this party's half in
/// `signed_funding_psbt`.
pub fn finalize_sign_spliced(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    signed_funding_psbt: &Psbt,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<Transaction, ContractError> {
    let request = finalize_request(offer, accept, sign)?;
    let mut signed_funding_psbt = signed_funding_psbt.clone();
    sign_splice_inputs(request.funding(), dlc_input_keys, &mut signed_funding_psbt)?;
    complete_finalize(request, &signed_funding_psbt)
}

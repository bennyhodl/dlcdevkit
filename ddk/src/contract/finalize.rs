//! Funding transaction completion by the accepting party.

use bitcoin::psbt::Psbt;
use bitcoin::Transaction;
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_dlc::secp256k1_zkp::{PublicKey, Secp256k1};
use ddk_messages::{AcceptDlc, FundingSignatures, OfferDlc, SignDlc};

use super::context::{apply_funding_signatures, funding_input_index, splice_half};
use super::error::ContractError;
use super::psbt::{ensure_psbt_matches_funding_transaction, extract_funding_signatures};
use super::types::{DlcInputSigningKey, Party};
use super::verify::{verify_signed_contract, VerifiedContract};

/// Verifies the sign message and completes the funding transaction.
///
/// The three messages are checked with
/// [`verify_signed_contract`](super::verify_signed_contract) first, so a sign
/// message with a bad signature or funding witness is rejected before any
/// transaction is returned.
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
    finalize_sign_spliced(offer, accept, sign, signed_funding_psbt, &[])
}

/// Verifies the sign message and completes the funding transaction, including
/// any splice (DLC) funding inputs.
///
/// Behaves like [`finalize_sign`] for ordinary funding inputs. For each DLC
/// (splice) input in the offer, `dlc_input_keys` must supply this (accepting)
/// party's previous contract funding secret key (matched by serial id). The
/// offering party's half signature is verified before this party's half is
/// produced and the two are combined into the input's final 2-of-2 witness.
/// Pass an empty slice when the contract has no splice inputs.
pub fn finalize_sign_spliced(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    signed_funding_psbt: &Psbt,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<Transaction, ContractError> {
    let contract = verify_signed_contract(offer, accept, sign)?;
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &contract.transactions().fund)?;
    let funding_signatures =
        extract_funding_signatures(offer, accept, Party::Accept, signed_funding_psbt)?;
    complete_funding_transaction(
        offer,
        accept,
        sign,
        &contract,
        funding_signatures,
        dlc_input_keys,
    )
}

/// Completes the funding transaction from already extracted accept-side witnesses.
pub(crate) fn finalize_sign_internal(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_signatures: FundingSignatures,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<Transaction, ContractError> {
    let contract = verify_signed_contract(offer, accept, sign)?;
    complete_funding_transaction(
        offer,
        accept,
        sign,
        &contract,
        funding_signatures,
        dlc_input_keys,
    )
}

/// Adds both parties' funding witnesses to the verified contract's funding
/// transaction: the offering party's from the sign message, this party's from
/// `funding_signatures`, and the combined 2-of-2 witness of each splice input.
fn complete_funding_transaction(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    contract: &VerifiedContract,
    funding_signatures: FundingSignatures,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<Transaction, ContractError> {
    if funding_signatures.funding_signatures.len() != accept.funding_inputs.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} accept funding signatures, received {}",
            accept.funding_inputs.len(),
            funding_signatures.funding_signatures.len()
        )));
    }
    let mut funding_transaction = contract.transactions().fund.clone();
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
    complete_dlc_input_witnesses(
        &mut funding_transaction,
        &contract.transactions().fund,
        offer,
        accept,
        sign,
        dlc_input_keys,
    )?;
    Ok(funding_transaction)
}

/// Combines the offering party's DLC-input half signatures, already verified
/// by [`verify_signed_contract`], with this (accepting) party's half to
/// complete each splice input's 2-of-2 witness on the funding transaction.
///
/// This party's half is computed over the unsigned funding transaction (the
/// SegWit sighash does not commit to other inputs' witnesses), matching what
/// the offering party signed.
fn complete_dlc_input_witnesses(
    funding_transaction: &mut Transaction,
    unsigned_funding_transaction: &Transaction,
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<(), ContractError> {
    let secp = Secp256k1::new();
    for (input, offer_signature) in offer
        .funding_inputs
        .iter()
        .zip(&sign.funding_signatures.funding_signatures)
    {
        let Some(dlc_input) = &input.dlc_input else {
            continue;
        };
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        let dlc_input_info: DlcInputInfo = input.into();
        let signing_key = dlc_input_keys
            .iter()
            .find(|key| key.input_serial_id == input.input_serial_id)
            .ok_or_else(|| {
                ContractError::InvalidFundingInput(format!(
                    "missing prior funding secret key for DLC input serial id {}",
                    input.input_serial_id
                ))
            })?;
        if PublicKey::from_secret_key(&secp, &signing_key.prior_funding_secret_key)
            != dlc_input.remote_fund_pubkey
        {
            return Err(ContractError::InvalidFundingInput(
                "prior funding secret key does not match the DLC input remote funding public key"
                    .to_string(),
            ));
        }
        let accept_half = ddk_dlc::dlc_input::create_dlc_funding_input_signature(
            &secp,
            unsigned_funding_transaction,
            input_index,
            &dlc_input_info,
            &signing_key.prior_funding_secret_key,
        )?;
        funding_transaction.input[input_index].witness =
            ddk_dlc::dlc_input::combine_dlc_input_signatures(
                &dlc_input_info,
                &accept_half,
                splice_half(input.input_serial_id, dlc_input, offer_signature)?,
                &dlc_input.remote_fund_pubkey,
                &dlc_input.local_fund_pubkey,
            )?;
    }
    Ok(())
}

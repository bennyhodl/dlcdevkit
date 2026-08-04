//! Sign message creation by the offering party.

use bitcoin::psbt::Psbt;
use bitcoin::{Transaction, Witness};
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_dlc::secp256k1_zkp::{All, PublicKey, Secp256k1, SecretKey};
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, FundingSignatures, OfferDlc, SignDlc};

use super::context::{
    context_from_messages, contract_id_from_transactions, create_adaptor_signatures,
    create_refund_signature, ensure_funding_key, funding_input_index,
    verify_counterparty_signatures, ContractContext,
};
use super::error::ContractError;
use super::psbt::{ensure_psbt_matches_funding_transaction, funding_signature_from_witness};
use super::types::{DlcInputSigningKey, SignResult};

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
    sign_accept_spliced(offer, accept, funding_secret_key, signed_funding_psbt, &[])
}

/// Verifies the accept message and creates the offering party's sign message,
/// including any splice (DLC) funding inputs.
///
/// Behaves like [`sign_accept`] for ordinary funding inputs. For each DLC
/// (splice) input in the offer, `dlc_input_keys` must supply the previous
/// contract's funding secret key (matched by serial id); this party produces
/// its half of the prior 2-of-2 signature, which the accepting party verifies
/// and completes in [`finalize_sign_spliced`](super::finalize_sign_spliced).
/// Pass an empty slice when the contract has no splice inputs.
pub fn sign_accept_spliced(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    signed_funding_psbt: &Psbt,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<SignResult, ContractError> {
    let context = context_from_messages(offer, accept)?;
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &context.transactions.fund)?;
    let secp = Secp256k1::new();
    let funding_signatures = build_offer_funding_signatures(
        offer,
        accept,
        &context.transactions.fund,
        signed_funding_psbt,
        dlc_input_keys,
        &secp,
    )?;
    sign_with_context(
        offer,
        accept,
        funding_secret_key,
        funding_signatures,
        context,
    )
}

/// Assembles the offering party's funding signatures in message order.
///
/// Ordinary inputs contribute their finalized PSBT witness; each DLC (splice)
/// input contributes a single-element witness holding this party's half of the
/// prior 2-of-2 signature, produced with the supplied prior funding secret key.
fn build_offer_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_transaction: &Transaction,
    signed_funding_psbt: &Psbt,
    dlc_input_keys: &[DlcInputSigningKey],
    secp: &Secp256k1<All>,
) -> Result<FundingSignatures, ContractError> {
    let mut funding_signatures = Vec::with_capacity(offer.funding_inputs.len());
    for input in &offer.funding_inputs {
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        if let Some(dlc_input) = &input.dlc_input {
            let signing_key = dlc_input_keys
                .iter()
                .find(|key| key.input_serial_id == input.input_serial_id)
                .ok_or_else(|| {
                    ContractError::InvalidFundingInput(format!(
                        "missing prior funding secret key for DLC input serial id {}",
                        input.input_serial_id
                    ))
                })?;
            if PublicKey::from_secret_key(secp, &signing_key.prior_funding_secret_key)
                != dlc_input.local_fund_pubkey
            {
                return Err(ContractError::InvalidFundingInput(
                    "prior funding secret key does not match the DLC input local funding public key"
                        .to_string(),
                ));
            }
            let dlc_input_info: DlcInputInfo = input.into();
            let signature = ddk_dlc::dlc_input::create_dlc_funding_input_signature(
                secp,
                funding_transaction,
                input_index,
                &dlc_input_info,
                &signing_key.prior_funding_secret_key,
            )?;
            funding_signatures.push(funding_signature_from_witness(Witness::from_slice(&[
                signature,
            ])));
        } else {
            let witness = signed_funding_psbt.inputs[input_index]
                .final_script_witness
                .clone()
                .filter(|witness| !witness.is_empty())
                .ok_or(ContractError::MissingFinalizedInput { input_index })?;
            funding_signatures.push(funding_signature_from_witness(witness));
        }
    }
    Ok(FundingSignatures { funding_signatures })
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

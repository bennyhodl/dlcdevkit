//! Low-level building blocks for advanced integrations.
//!
//! Most consumers should use the primary lifecycle functions in
//! [`ddk::contract`](super) together with the [`signing`](super::signing)
//! sources. The functions here expose the raw adaptor-signature and witness
//! plumbing for integrations that interoperate with other DLC implementations
//! or produce funding witnesses outside of a PSBT.

use bitcoin::psbt::Psbt;
use bitcoin::sighash::EcdsaSighashType;
use bitcoin::{Amount, Transaction, Witness};
use ddk_dlc::secp256k1_zkp::{EcdsaAdaptorSignature, Secp256k1, SecretKey};
use ddk_messages::{
    AcceptDlc, CetAdaptorSignatures, FundingSignature, FundingSignatures, OfferDlc, SignDlc,
};

use super::context::{self, context_from_messages};
use super::error::ContractError;
use super::psbt;
use super::types::{Party, SignResult};

/// Converts a Bitcoin witness into a wire funding signature.
pub fn funding_signature_from_witness(witness: Witness) -> FundingSignature {
    psbt::funding_signature_from_witness(witness)
}

/// Converts Bitcoin witnesses into wire funding signatures.
///
/// The witnesses must be ordered like the party's funding inputs in its wire
/// message.
pub fn funding_signatures_from_witnesses(witnesses: Vec<Witness>) -> FundingSignatures {
    FundingSignatures {
        funding_signatures: witnesses
            .into_iter()
            .map(psbt::funding_signature_from_witness)
            .collect(),
    }
}

/// Signs one native P2WPKH funding input and returns its wire-format witness.
pub fn sign_p2wpkh_funding_input(
    funding_transaction: &Transaction,
    input_index: usize,
    prevout_value: Amount,
    secret_key: &SecretKey,
) -> Result<FundingSignature, ContractError> {
    let secp = Secp256k1::new();
    let witness = ddk_dlc::util::get_witness_for_p2wpkh_input(
        &secp,
        secret_key,
        funding_transaction,
        input_index,
        EcdsaSighashType::All,
        prevout_value,
    )?;
    Ok(psbt::funding_signature_from_witness(witness))
}

/// Creates one party's CET adaptor signatures over all contract outcomes.
pub fn create_cet_adaptor_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
) -> Result<Vec<EcdsaAdaptorSignature>, ContractError> {
    let secp = Secp256k1::new();
    let context = context_from_messages(offer, accept)?;
    context::create_adaptor_signatures(
        &secp,
        &context,
        funding_secret_key,
        offer.get_total_collateral(),
    )
}

/// Verifies one party's refund and CET adaptor signatures.
///
/// `party` names the party that produced the signatures.
pub fn verify_cet_adaptor_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    party: Party,
    refund_signature: &ddk_dlc::secp256k1_zkp::ecdsa::Signature,
    adaptor_signatures: &CetAdaptorSignatures,
) -> Result<(), ContractError> {
    let secp = Secp256k1::new();
    let context = context_from_messages(offer, accept)?;
    let (funding_pubkey, error): (_, fn(String) -> ContractError) = match party {
        Party::Offer => (offer.funding_pubkey, ContractError::InvalidSign),
        Party::Accept => (accept.funding_pubkey, ContractError::InvalidAccept),
    };
    context::verify_counterparty_signatures(
        &secp,
        &context,
        offer.get_total_collateral(),
        funding_pubkey,
        refund_signature,
        adaptor_signatures,
        error,
    )
}

/// Extracts one party's finalized funding witnesses from a funding PSBT.
///
/// The PSBT is first verified against the funding transaction rebuilt from
/// the messages.
pub fn funding_signatures_from_psbt(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    party: Party,
    psbt: &Psbt,
) -> Result<FundingSignatures, ContractError> {
    psbt::ensure_matching_psbt(offer, accept, psbt)?;
    psbt::extract_funding_signatures(offer, accept, party, psbt)
}

/// Computes the contract id from the offer and accept messages.
pub fn compute_contract_id(
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> Result<[u8; 32], ContractError> {
    let context = context_from_messages(offer, accept)?;
    Ok(context::contract_id_from_transactions(
        &context.transactions,
        &offer.temporary_contract_id,
    ))
}

/// Creates the sign message from externally produced offer-side funding witnesses.
///
/// Prefer [`sign_accept`](super::sign_accept) with a PSBT; this variant exists
/// for integrations that already hold raw witnesses. `funding_signatures` must
/// contain one witness per offer funding input, in message order.
pub fn sign_accept_with_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    funding_signatures: FundingSignatures,
) -> Result<SignResult, ContractError> {
    super::sign::sign_accept_internal(offer, accept, funding_secret_key, funding_signatures)
}

/// Completes the funding transaction from externally produced accept-side witnesses.
///
/// Prefer [`finalize_sign`](super::finalize_sign) with a PSBT; this variant
/// exists for integrations that already hold raw witnesses.
/// `funding_signatures` must contain one witness per accept funding input, in
/// message order.
pub fn finalize_sign_with_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_signatures: FundingSignatures,
) -> Result<Transaction, ContractError> {
    super::finalize::finalize_sign_internal(offer, accept, sign, funding_signatures)
}

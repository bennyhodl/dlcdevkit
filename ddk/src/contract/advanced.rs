//! Low-level building blocks for advanced integrations.
//!
//! Most consumers should use the primary lifecycle functions in
//! [`ddk::contract`](super) together with the [`signing`](super::signing)
//! sources, or [`external`](super::external) for keys held outside the
//! process. The functions here expose the raw adaptor-signature, witness and
//! identifier plumbing for integrations that interoperate with other DLC
//! implementations, produce funding witnesses outside of a PSBT, or build their
//! own signer.

use bitcoin::psbt::Psbt;
use bitcoin::sighash::EcdsaSighashType;
use bitcoin::{Amount, Transaction, Witness};
use ddk_dlc::secp256k1_zkp::{EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey};
use ddk_messages::{
    AcceptDlc, CetAdaptorSignatures, FundingSignature, FundingSignatures, OfferDlc, SignDlc,
};

use super::context::{self, context_from_messages};
use super::error::ContractError;
use super::external::{self, ContractSigningRequest};
use super::psbt;
use super::types::{DlcInputSigningKey, Party, SignResult};

pub use super::context::{decode_previous_transaction, funding_input_index};
pub use ddk_manager::contract::{contract_id_from_outpoint, temporary_contract_id_from_outpoint};

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

/// The adaptor point of every CET adaptor signature the offer's contract
/// needs, in signature order, each with the index of the CET it signs.
///
/// Both parties sign the same CETs to the same points, and the points depend
/// on the offer alone: a signer can compute them before the accept message
/// exists. CET indexes run over every CET of the contract, in the order
/// [`create_dlc_transactions`](super::create_dlc_transactions) returns them.
/// This is the [`ContractSigningRequest::adaptor_points`] the
/// [`external`](super::external) requests carry.
pub fn cet_adaptor_points(offer: &OfferDlc) -> Result<Vec<(usize, PublicKey)>, ContractError> {
    let execution_infos = ddk_manager::contract::execution_contract_infos(&offer.contract_info)?;
    context::cet_adaptor_points(&execution_infos, offer.get_total_collateral())
}

/// Creates one party's CET adaptor signatures over all contract outcomes.
pub fn create_cet_adaptor_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
) -> Result<Vec<EcdsaAdaptorSignature>, ContractError> {
    let context = context_from_messages(offer, accept)?;
    let request = ContractSigningRequest::new(
        &context.transactions,
        context::cet_adaptor_points(&context.execution_infos, offer.get_total_collateral())?,
        funding_secret_key.public_key(&Secp256k1::new()),
    )?;
    Ok(
        external::sign_contract(&request, funding_secret_key, ContractError::Key)?
            .cet_adaptor_signatures,
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
    let context = context_from_messages(offer, accept)?;
    let (funding_pubkey, error): (_, fn(String) -> ContractError) = match party {
        Party::Offer => (offer.funding_pubkey, ContractError::InvalidSign),
        Party::Accept => (accept.funding_pubkey, ContractError::InvalidAccept),
    };
    context::verify_contract_signatures(
        &Secp256k1::new(),
        &context.transactions,
        &context::cet_adaptor_points(&context.execution_infos, offer.get_total_collateral())?,
        &funding_pubkey,
        refund_signature,
        &Vec::from(adaptor_signatures),
        error,
    )
}

/// Extracts one party's funding signatures from a funding PSBT: a finalized
/// witness per wallet input, and the party's half signature per splice input.
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
/// contain one witness per offer funding input, in message order; a splice
/// input's is its single half signature.
pub fn sign_accept_with_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
    funding_signatures: FundingSignatures,
) -> Result<SignResult, ContractError> {
    let request = external::sign_request(offer, accept)?;
    let signatures = external::sign_contract(
        request.contract(),
        funding_secret_key,
        ContractError::InvalidOffer,
    )?;
    external::complete_sign_with_funding_signatures(request, signatures, funding_signatures)
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
    finalize_sign_spliced_with_funding_signatures(offer, accept, sign, funding_signatures, &[])
}

/// Splice-aware variant of [`finalize_sign_with_funding_signatures`].
///
/// `dlc_input_keys` supplies this (accepting) party's previous contract funding
/// secret key for each DLC (splice) funding input in the offer, matched by
/// serial id. The offering party's DLC-input half signatures must already be
/// present in `sign.funding_signatures`.
pub fn finalize_sign_spliced_with_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_signatures: FundingSignatures,
    dlc_input_keys: &[DlcInputSigningKey],
) -> Result<Transaction, ContractError> {
    let request = external::finalize_request(offer, accept, sign)?;
    let mut splice_signatures = request.funding().psbt.clone();
    external::sign_splice_inputs(request.funding(), dlc_input_keys, &mut splice_signatures)?;
    external::complete_finalize_with_funding_signatures(
        request,
        funding_signatures,
        &splice_signatures,
    )
}

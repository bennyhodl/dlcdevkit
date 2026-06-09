//! Funding PSBT construction, validation, finalization, and witness extraction.
//!
//! The PSBT is the universal signing boundary for funding inputs: every
//! signing source (wallet, xpriv, descriptor, or external signer) produces
//! finalized witnesses inside a PSBT, and the lifecycle functions extract wire
//! [`FundingSignatures`] from it. PSBTs never contain private key material.

use bitcoin::psbt::Psbt;
use bitcoin::script::PushBytesBuf;
use bitcoin::sighash::EcdsaSighashType;
use bitcoin::{ScriptBuf, Transaction, Witness};
use ddk_messages::{AcceptDlc, FundingSignature, FundingSignatures, OfferDlc, WitnessElement};

use super::context::{
    context_from_messages, decode_previous_transaction, funding_input_index, party_funding_inputs,
};
use super::error::ContractError;
use super::types::Party;

/// Builds the funding PSBT from the offer and accept messages.
///
/// The PSBT contains, for every funding input: the `witness_utxo` (for SegWit
/// inputs), the `non_witness_utxo` (the full previous transaction, which some
/// signers require), the redeem script for P2SH-wrapped inputs, and the
/// `SIGHASH_ALL` sighash type. Input order follows ascending funding input
/// serial ids, matching the funding transaction.
pub fn create_funding_psbt(offer: &OfferDlc, accept: &AcceptDlc) -> Result<Psbt, ContractError> {
    let transactions = context_from_messages(offer, accept)?.transactions;
    build_funding_psbt(offer, accept, transactions.fund)
}

/// Builds the funding PSBT from an already rebuilt funding transaction.
pub(crate) fn build_funding_psbt(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    mut funding_transaction: Transaction,
) -> Result<Psbt, ContractError> {
    // PSBT unsigned transactions must have empty script sigs; the P2SH-P2WPKH
    // redeem script push is restored by the input finalizer.
    for input in &mut funding_transaction.input {
        input.script_sig = ScriptBuf::new();
    }
    let mut psbt = Psbt::from_unsigned_tx(funding_transaction)
        .map_err(|e| ContractError::PsbtMismatch(format!("could not create PSBT: {e}")))?;

    for input in offer.funding_inputs.iter().chain(&accept.funding_inputs) {
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        let previous_transaction = decode_previous_transaction(input)?;
        let outpoint = psbt.unsigned_tx.input[input_index].previous_output;
        if outpoint.txid != previous_transaction.compute_txid()
            || outpoint.vout != input.prev_tx_vout
        {
            return Err(ContractError::InvalidFundingInput(format!(
                "funding input serial id {} does not match the funding transaction outpoint",
                input.input_serial_id
            )));
        }
        let prevout = previous_transaction
            .output
            .get(input.prev_tx_vout as usize)
            .ok_or_else(|| {
                ContractError::InvalidFundingInput(format!(
                    "previous output {} does not exist",
                    input.prev_tx_vout
                ))
            })?;

        let script_pubkey = &prevout.script_pubkey;
        if script_pubkey.is_p2sh() {
            if input.redeem_script.is_empty() {
                return Err(ContractError::InvalidFundingInput(format!(
                    "funding input serial id {} is P2SH but has no redeem script",
                    input.input_serial_id
                )));
            }
            if ScriptBuf::new_p2sh(&input.redeem_script.script_hash()) != *script_pubkey {
                return Err(ContractError::InvalidFundingInput(format!(
                    "funding input serial id {} redeem script does not match the script pubkey",
                    input.input_serial_id
                )));
            }
            psbt.inputs[input_index].redeem_script = Some(input.redeem_script.clone());
        } else if !input.redeem_script.is_empty() {
            return Err(ContractError::InvalidFundingInput(format!(
                "funding input serial id {} has a redeem script for a non-P2SH output",
                input.input_serial_id
            )));
        }

        let is_segwit = script_pubkey.is_witness_program()
            || (script_pubkey.is_p2sh() && input.redeem_script.is_witness_program());
        if is_segwit {
            psbt.inputs[input_index].witness_utxo = Some(prevout.clone());
        }
        psbt.inputs[input_index].non_witness_utxo = Some(previous_transaction);
        psbt.inputs[input_index].sighash_type = Some(EcdsaSighashType::All.into());
    }

    Ok(psbt)
}

/// Verifies that a PSBT spends exactly the rebuilt funding transaction.
pub(crate) fn ensure_psbt_matches_funding_transaction(
    psbt: &Psbt,
    funding_transaction: &Transaction,
) -> Result<(), ContractError> {
    let mut expected = funding_transaction.clone();
    for input in &mut expected.input {
        input.script_sig = ScriptBuf::new();
    }
    if psbt.unsigned_tx != expected {
        return Err(ContractError::PsbtMismatch(
            "PSBT unsigned transaction does not match the funding transaction rebuilt from the \
             offer and accept messages"
                .to_string(),
        ));
    }
    if psbt.inputs.len() != expected.input.len() {
        return Err(ContractError::PsbtMismatch(format!(
            "PSBT has {} inputs but the funding transaction has {}",
            psbt.inputs.len(),
            expected.input.len()
        )));
    }
    Ok(())
}

/// Rebuilds the funding transaction from the messages and verifies the PSBT
/// against it.
pub(crate) fn ensure_matching_psbt(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    psbt: &Psbt,
) -> Result<(), ContractError> {
    let transactions = context_from_messages(offer, accept)?.transactions;
    ensure_psbt_matches_funding_transaction(psbt, &transactions.fund)
}

/// Extracts one party's finalized funding witnesses from a PSBT.
///
/// The PSBT must already be verified against the rebuilt funding transaction.
pub(crate) fn extract_funding_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    party: Party,
    psbt: &Psbt,
) -> Result<FundingSignatures, ContractError> {
    let funding_signatures = party_funding_inputs(offer, accept, party)
        .iter()
        .map(|input| {
            let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
            let witness = psbt.inputs[input_index]
                .final_script_witness
                .clone()
                .filter(|witness| !witness.is_empty())
                .ok_or(ContractError::MissingFinalizedInput { input_index })?;
            Ok(funding_signature_from_witness(witness))
        })
        .collect::<Result<Vec<_>, ContractError>>()?;

    Ok(FundingSignatures { funding_signatures })
}

/// Finalizes a signed P2WPKH or P2SH-P2WPKH PSBT input.
///
/// Looks for a partial signature matching the input's script and converts it
/// into a finalized witness. Other script types return
/// [`ContractError::UnsupportedScriptType`].
pub(crate) fn finalize_segwit_input(
    psbt: &mut Psbt,
    input_index: usize,
) -> Result<(), ContractError> {
    let input = psbt.inputs.get_mut(input_index).ok_or_else(|| {
        ContractError::PsbtMismatch(format!("PSBT input {input_index} does not exist"))
    })?;
    if input.final_script_witness.is_some() {
        return Ok(());
    }

    let script_pubkey = input
        .witness_utxo
        .as_ref()
        .ok_or_else(|| {
            ContractError::PsbtMismatch(format!(
                "PSBT input {input_index} is missing its witness UTXO"
            ))
        })?
        .script_pubkey
        .clone();

    // Resolve the P2WPKH program, whether native or P2SH-wrapped.
    let (witness_script_pubkey, redeem_script) = if script_pubkey.is_p2wpkh() {
        (script_pubkey, None)
    } else if script_pubkey.is_p2sh() {
        let redeem_script = input.redeem_script.clone().ok_or_else(|| {
            ContractError::PsbtMismatch(format!(
                "PSBT input {input_index} is P2SH but has no redeem script"
            ))
        })?;
        if !redeem_script.is_p2wpkh() {
            return Err(ContractError::UnsupportedScriptType { input_index });
        }
        (redeem_script.clone(), Some(redeem_script))
    } else {
        return Err(ContractError::UnsupportedScriptType { input_index });
    };

    let (public_key, signature) = input
        .partial_sigs
        .iter()
        .find_map(|(public_key, signature)| {
            public_key
                .wpubkey_hash()
                .ok()
                .filter(|hash| ScriptBuf::new_p2wpkh(hash) == witness_script_pubkey)
                .map(|_| (*public_key, *signature))
        })
        .ok_or_else(|| {
            ContractError::InvalidFundingInput(format!(
                "PSBT input {input_index} does not have a signature matching its script"
            ))
        })?;

    input.final_script_witness = Some(Witness::from_slice(&[
        signature.to_vec(),
        public_key.to_bytes(),
    ]));
    if let Some(redeem_script) = redeem_script {
        let push = PushBytesBuf::try_from(redeem_script.into_bytes()).map_err(|_| {
            ContractError::InvalidFundingInput(format!(
                "PSBT input {input_index} redeem script is too long"
            ))
        })?;
        input.final_script_sig = Some(ScriptBuf::builder().push_slice(push).into_script());
    }
    input.partial_sigs.clear();
    Ok(())
}

/// Converts a Bitcoin witness into a wire funding signature.
pub(crate) fn funding_signature_from_witness(witness: Witness) -> FundingSignature {
    FundingSignature {
        witness_elements: witness
            .iter()
            .map(|element| WitnessElement {
                witness: element.to_vec(),
            })
            .collect(),
    }
}

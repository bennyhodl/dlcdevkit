//! Splice (DLC) funding input construction.
//!
//! A splice reuses the 2-of-2 funding output of a previous, on-chain DLC as an
//! input to a new contract's funding transaction (this is how rollovers and
//! collateral changes are expressed). [`create_dlc_splice_input`] rebuilds that
//! output from the previous contract's offer, accept and sign messages, so
//! callers never have to supply raw transaction data.
//!
//! Only the offering party may contribute a DLC input; the accepting party's
//! funding inputs must be ordinary wallet UTXOs.

use bitcoin::sighash::EcdsaSighashType;
use bitcoin::{OutPoint, ScriptBuf, Transaction};
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_dlc::secp256k1_zkp::{All, PublicKey, Secp256k1};
use ddk_messages::{AcceptDlc, DlcInput, FundingInput, OfferDlc, SignDlc};

use super::accept::create_signed_dlc_transactions;
use super::context::decode_previous_transaction;
use super::error::ContractError;
use super::types::{random_serial_id, Party};

/// Maximum witness length reported for a DLC (2-of-2 P2WSH) funding input.
///
/// Must exceed 108 so that [`ddk_dlc::create_spliced_dlc_transactions`]
/// separates DLC inputs from ordinary P2WPKH wallet inputs. The value matches
/// `ddk-manager`, so a stateful counterparty rebuilds an identical funding
/// transaction.
pub const DLC_INPUT_MAX_WITNESS_LEN: u16 = 220;

/// Builds a splice (DLC) funding input from a previous contract's messages.
///
/// The previous contract must be the funded DLC whose 2-of-2 funding output is
/// spent into the new contract. `local_party` identifies which side of the
/// previous contract the *new offering party* was; offer-only splicing uses
/// [`Party::Offer`].
///
/// The returned [`FundingInput`] carries the previous funding transaction, its
/// funding output index, and a [`DlcInput`] with both prior funding public keys
/// and the prior contract id. Place it in the offering party's funding inputs
/// when building the new offer. Signing it later additionally requires the
/// prior contract's funding secret key (see
/// [`sign_accept_spliced`](super::sign_accept_spliced) and
/// [`finalize_sign_spliced`](super::finalize_sign_spliced)).
pub fn create_dlc_splice_input(
    prev_offer: &OfferDlc,
    prev_accept: &AcceptDlc,
    prev_sign: &SignDlc,
    local_party: Party,
    input_serial_id: Option<u64>,
    max_witness_len: u16,
) -> Result<FundingInput, ContractError> {
    if max_witness_len as usize <= 108 {
        return Err(ContractError::InvalidFundingInput(
            "DLC input max witness length must be greater than 108".to_string(),
        ));
    }
    let transactions = create_signed_dlc_transactions(prev_offer, prev_accept, prev_sign)?;
    let fund_vout = transactions.get_fund_output_index() as u32;
    let contract_id = prev_sign.contract_id;
    let (local_fund_pubkey, remote_fund_pubkey) = match local_party {
        Party::Offer => (prev_offer.funding_pubkey, prev_accept.funding_pubkey),
        Party::Accept => (prev_accept.funding_pubkey, prev_offer.funding_pubkey),
    };
    Ok(FundingInput {
        input_serial_id: input_serial_id.unwrap_or_else(random_serial_id),
        prev_tx: bitcoin::consensus::serialize(&transactions.fund),
        prev_tx_vout: fund_vout,
        sequence: u32::MAX,
        max_witness_len,
        redeem_script: ScriptBuf::new(),
        dlc_input: Some(DlcInput {
            local_fund_pubkey,
            remote_fund_pubkey,
            contract_id,
        }),
    })
}

/// The temporary id of the contract a splice input spends, from the input
/// alone: the input names that contract by contract id and carries its
/// funding transaction.
///
/// The funding keys of the previous contract derive from its temporary id, so
/// this is what lets a key provider sign a splice input it holds no record of.
pub(crate) fn prior_temporary_contract_id(
    input: &FundingInput,
    dlc_input: &DlcInput,
) -> Result<[u8; 32], ContractError> {
    let funding_outpoint = OutPoint::new(
        decode_previous_transaction(input)?.compute_txid(),
        input.prev_tx_vout,
    );
    Ok(ddk_manager::contract::temporary_contract_id_from_outpoint(
        funding_outpoint,
        &dlc_input.contract_id,
    ))
}

/// Verifies one party's half of the 2-of-2 signature spending a splice input:
/// a DER signature followed by its sighash byte, by `funding_pubkey`, over the
/// unsigned funding transaction.
///
/// Only `SIGHASH_ALL` is accepted. The byte is part of what goes on chain, and
/// a half signed with another type would commit to less than the whole
/// funding transaction.
pub(crate) fn verify_splice_signature(
    secp: &Secp256k1<All>,
    funding_transaction: &Transaction,
    input_index: usize,
    input: &FundingInput,
    signature: &[u8],
    funding_pubkey: &PublicKey,
) -> Result<(), String> {
    let signature = bitcoin::ecdsa::Signature::from_slice(signature).map_err(|e| {
        format!("splice input {input_index}: not a DER signature with a sighash byte: {e}")
    })?;
    if signature.sighash_type != EcdsaSighashType::All {
        return Err(format!(
            "splice input {input_index} is signed with {}, SIGHASH_ALL is required",
            signature.sighash_type
        ));
    }
    let dlc_input: DlcInputInfo = input.into();
    ddk_dlc::verify_tx_input_sig(
        secp,
        &signature.signature,
        funding_transaction,
        input_index,
        &ddk_dlc::dlc_input::create_dlc_input_funding_script(&dlc_input),
        dlc_input.fund_amount,
        funding_pubkey,
    )
    .map_err(|e| {
        format!(
            "splice input {input_index}: the signature by {funding_pubkey} does not verify: {e}"
        )
    })
}

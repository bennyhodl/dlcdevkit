//! Internal reconstruction and validation of contract state from wire messages.
//!
//! Nothing in this module is persisted. Every lifecycle operation rebuilds the
//! transactions it needs from the offer and accept messages so that callers
//! never have to supply, store, or trust intermediate transaction data.

use bitcoin::consensus::Decodable;
use bitcoin::{Amount, ScriptBuf, Transaction, Witness};
use ddk_dlc::secp256k1_zkp::{All, EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey};
use ddk_dlc::{DlcTransactions, PartyParams as DlcPartyParams, TxInputInfo};
use ddk_manager::contract::contract_info::ContractInfo as ExecutionContractInfo;
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, FundingInput, FundingSignatures, OfferDlc};

use super::error::ContractError;
use super::types::Party;
use super::PROTOCOL_VERSION;

/// Contract data rebuilt from the offer and accept messages.
pub(crate) struct ContractContext {
    pub execution_infos: Vec<ExecutionContractInfo>,
    pub cet_ranges: Vec<std::ops::Range<usize>>,
    pub transactions: DlcTransactions,
}

/// Validates an offer/accept pair and rebuilds the contract transactions.
pub(crate) fn context_from_messages(
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> Result<ContractContext, ContractError> {
    ensure_protocol_version(offer.protocol_version, ContractError::InvalidOffer)?;
    ensure_protocol_version(accept.protocol_version, ContractError::InvalidAccept)?;
    if offer.protocol_version != accept.protocol_version {
        return Err(ContractError::InvalidAccept(
            "offer and accept protocol versions differ".to_string(),
        ));
    }
    if offer.temporary_contract_id != accept.temporary_contract_id {
        return Err(ContractError::InvalidAccept(
            "accept message references a different temporary contract id".to_string(),
        ));
    }
    if accept.negotiation_fields.is_some() {
        return Err(ContractError::InvalidAccept(
            "negotiation fields are not supported by the stateless API".to_string(),
        ));
    }
    ensure_no_dlc_inputs(&offer.funding_inputs)?;
    ensure_no_dlc_inputs(&accept.funding_inputs)?;
    ensure_unique_input_serial_ids(offer, accept)?;

    let accept_params = dlc_party_params(
        accept.funding_pubkey,
        accept.payout_spk.clone(),
        accept.payout_serial_id,
        accept.change_spk.clone(),
        accept.change_serial_id,
        accept.accept_collateral,
        &accept.funding_inputs,
    )?;
    build_context(offer, &accept_params)
}

/// Rebuilds the contract transactions from an offer and the accepting party's
/// parameters. Used by [`context_from_messages`] and by accept-message creation
/// before the accept message exists.
pub(crate) fn build_context(
    offer: &OfferDlc,
    accept_params: &DlcPartyParams,
) -> Result<ContractContext, ContractError> {
    let total_collateral = offer.get_total_collateral();
    if offer.offer_collateral + accept_params.collateral != total_collateral {
        return Err(ContractError::InvalidAccept(
            "offer and accept collateral do not equal total collateral".to_string(),
        ));
    }
    let offer_params = dlc_party_params(
        offer.funding_pubkey,
        offer.payout_spk.clone(),
        offer.payout_serial_id,
        offer.change_spk.clone(),
        offer.change_serial_id,
        offer.offer_collateral,
        &offer.funding_inputs,
    )?;
    let execution_infos = ddk_manager::contract::execution_contract_infos(&offer.contract_info)?;
    if execution_infos.is_empty() {
        return Err(ContractError::InvalidOffer(
            "contract does not contain execution information".to_string(),
        ));
    }
    for info in &execution_infos {
        info.validate()?;
    }

    let mut transactions = ddk_dlc::create_dlc_transactions(
        &offer_params,
        accept_params,
        &execution_infos[0].get_payouts(total_collateral)?,
        offer.refund_locktime,
        offer.fee_rate_per_vb,
        0,
        offer.cet_locktime,
        offer.fund_output_serial_id,
        offer.contract_flags,
    )?;
    let mut cet_ranges = Vec::with_capacity(execution_infos.len());
    cet_ranges.push(0..transactions.cets.len());
    let cet_input = transactions
        .cets
        .first()
        .ok_or_else(|| ContractError::InvalidOffer("contract has no CETs".to_string()))?
        .input[0]
        .clone();

    for info in execution_infos.iter().skip(1) {
        let start = transactions.cets.len();
        transactions.cets.extend(ddk_dlc::create_cets(
            &cet_input,
            &offer_params.payout_script_pubkey,
            offer_params.payout_serial_id,
            &accept_params.payout_script_pubkey,
            accept_params.payout_serial_id,
            &info.get_payouts(total_collateral)?,
            0,
        ));
        cet_ranges.push(start..transactions.cets.len());
    }

    Ok(ContractContext {
        execution_infos,
        cet_ranges,
        transactions,
    })
}

pub(crate) fn dlc_party_params(
    funding_pubkey: PublicKey,
    payout_script_pubkey: ScriptBuf,
    payout_serial_id: u64,
    change_script_pubkey: ScriptBuf,
    change_serial_id: u64,
    collateral: Amount,
    funding_inputs: &[FundingInput],
) -> Result<DlcPartyParams, ContractError> {
    let (inputs, input_amount) = tx_input_infos(funding_inputs)?;
    Ok(DlcPartyParams {
        fund_pubkey: funding_pubkey,
        change_script_pubkey,
        change_serial_id,
        payout_script_pubkey,
        payout_serial_id,
        inputs,
        dlc_inputs: vec![],
        input_amount,
        collateral,
    })
}

fn tx_input_infos(
    funding_inputs: &[FundingInput],
) -> Result<(Vec<TxInputInfo>, Amount), ContractError> {
    let mut input_amount = Amount::ZERO;
    let mut inputs = Vec::with_capacity(funding_inputs.len());
    for input in funding_inputs {
        let previous_transaction = decode_previous_transaction(input)?;
        let prevout = previous_transaction
            .output
            .get(input.prev_tx_vout as usize)
            .ok_or_else(|| {
                ContractError::InvalidFundingInput(format!(
                    "previous output {} does not exist",
                    input.prev_tx_vout
                ))
            })?;
        input_amount += prevout.value;
        inputs.push(TxInputInfo {
            outpoint: bitcoin::OutPoint {
                txid: previous_transaction.compute_txid(),
                vout: input.prev_tx_vout,
            },
            max_witness_len: input.max_witness_len as usize,
            redeem_script: input.redeem_script.clone(),
            serial_id: input.input_serial_id,
        });
    }
    Ok((inputs, input_amount))
}

/// Creates this party's CET adaptor signatures and groups them per execution info.
pub(crate) fn create_adaptor_signatures(
    secp: &Secp256k1<All>,
    context: &ContractContext,
    funding_secret_key: &SecretKey,
    total_collateral: Amount,
) -> Result<Vec<EcdsaAdaptorSignature>, ContractError> {
    let mut signatures = Vec::new();
    for (info, range) in context.execution_infos.iter().zip(&context.cet_ranges) {
        let (_, mut info_signatures) = info.get_adaptor_info(
            secp,
            total_collateral,
            funding_secret_key,
            &context.transactions.funding_script_pubkey,
            context.transactions.get_fund_output().value,
            &context.transactions.cets[range.clone()],
            signatures.len(),
        )?;
        signatures.append(&mut info_signatures);
    }
    Ok(signatures)
}

/// Creates this party's refund transaction signature.
pub(crate) fn create_refund_signature(
    secp: &Secp256k1<All>,
    context: &ContractContext,
    funding_secret_key: &SecretKey,
) -> Result<ddk_dlc::secp256k1_zkp::ecdsa::Signature, ContractError> {
    Ok(ddk_dlc::util::get_raw_sig_for_tx_input(
        secp,
        &context.transactions.refund,
        0,
        &context.transactions.funding_script_pubkey,
        context.transactions.get_fund_output().value,
        funding_secret_key,
    )?)
}

/// Verifies the counterparty's refund and CET adaptor signatures.
///
/// `error` attributes failures to the message that carried the signatures
/// (accept or sign).
pub(crate) fn verify_counterparty_signatures(
    secp: &Secp256k1<All>,
    context: &ContractContext,
    total_collateral: Amount,
    counterparty_funding_pubkey: PublicKey,
    refund_signature: &ddk_dlc::secp256k1_zkp::ecdsa::Signature,
    adaptor_signatures: &CetAdaptorSignatures,
    error: fn(String) -> ContractError,
) -> Result<(), ContractError> {
    let funding_value = context.transactions.get_fund_output().value;
    ddk_dlc::verify_tx_input_sig(
        secp,
        refund_signature,
        &context.transactions.refund,
        0,
        &context.transactions.funding_script_pubkey,
        funding_value,
        &counterparty_funding_pubkey,
    )
    .map_err(|e| error(format!("invalid refund signature: {e}")))?;

    let signatures: Vec<EcdsaAdaptorSignature> = adaptor_signatures.into();
    let mut signature_index = 0;
    for (info, range) in context.execution_infos.iter().zip(&context.cet_ranges) {
        let (_, next_index) = info
            .verify_and_get_adaptor_info(
                secp,
                total_collateral,
                &counterparty_funding_pubkey,
                &context.transactions.funding_script_pubkey,
                funding_value,
                &context.transactions.cets[range.clone()],
                &signatures,
                signature_index,
            )
            .map_err(|e| error(format!("invalid CET adaptor signatures: {e}")))?;
        signature_index = next_index;
    }
    if signature_index != signatures.len() {
        return Err(error(format!(
            "received {} adaptor signatures but used {}",
            signatures.len(),
            signature_index
        )));
    }
    Ok(())
}

/// Applies one party's funding witnesses to the funding transaction.
pub(crate) fn apply_funding_signatures(
    transaction: &mut Transaction,
    offer: &OfferDlc,
    accept: &AcceptDlc,
    party: Party,
    signatures: &FundingSignatures,
) -> Result<(), ContractError> {
    let inputs = party_funding_inputs(offer, accept, party);
    if inputs.len() != signatures.funding_signatures.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} funding signatures, received {}",
            inputs.len(),
            signatures.funding_signatures.len()
        )));
    }
    for (input, signature) in inputs.iter().zip(&signatures.funding_signatures) {
        if signature.witness_elements.is_empty() {
            return Err(ContractError::InvalidFundingInput(format!(
                "funding signature for input serial id {} has no witness elements",
                input.input_serial_id
            )));
        }
        let index = funding_input_index(offer, accept, input.input_serial_id)?;
        transaction.input[index].witness = Witness::from_slice(
            &signature
                .witness_elements
                .iter()
                .map(|element| element.witness.clone())
                .collect::<Vec<_>>(),
        );
    }
    Ok(())
}

/// Maps a funding input serial id to its index in the funding transaction.
///
/// Funding inputs are ordered by ascending serial id across both parties.
pub(crate) fn funding_input_index(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    input_serial_id: u64,
) -> Result<usize, ContractError> {
    let mut serial_ids = offer
        .funding_inputs
        .iter()
        .chain(&accept.funding_inputs)
        .map(|input| input.input_serial_id)
        .collect::<Vec<_>>();
    if serial_ids
        .iter()
        .filter(|id| **id == input_serial_id)
        .count()
        != 1
    {
        return Err(ContractError::InvalidFundingInput(format!(
            "funding input serial id {input_serial_id} is not unique"
        )));
    }
    serial_ids.sort_unstable();
    serial_ids
        .iter()
        .position(|id| *id == input_serial_id)
        .ok_or_else(|| {
            ContractError::InvalidFundingInput(format!(
                "funding input serial id {input_serial_id} was not found"
            ))
        })
}

pub(crate) fn party_funding_inputs<'a>(
    offer: &'a OfferDlc,
    accept: &'a AcceptDlc,
    party: Party,
) -> &'a [FundingInput] {
    match party {
        Party::Offer => &offer.funding_inputs,
        Party::Accept => &accept.funding_inputs,
    }
}

pub(crate) fn ensure_protocol_version(
    version: u32,
    error: fn(String) -> ContractError,
) -> Result<(), ContractError> {
    if version != PROTOCOL_VERSION {
        return Err(error(format!("unsupported DLC protocol version {version}")));
    }
    Ok(())
}

pub(crate) fn ensure_funding_key(
    secp: &Secp256k1<All>,
    secret_key: &SecretKey,
    expected_public_key: &PublicKey,
    error: fn(String) -> ContractError,
) -> Result<(), ContractError> {
    if PublicKey::from_secret_key(secp, secret_key) != *expected_public_key {
        return Err(error(
            "funding secret key does not match the funding public key".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_no_dlc_inputs(funding_inputs: &[FundingInput]) -> Result<(), ContractError> {
    if funding_inputs.iter().any(|input| input.dlc_input.is_some()) {
        return Err(ContractError::InvalidFundingInput(
            "DLC inputs and splicing require persisted previous-contract state".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_unique_input_serial_ids(
    offer: &OfferDlc,
    accept: &AcceptDlc,
) -> Result<(), ContractError> {
    let mut serial_ids = offer
        .funding_inputs
        .iter()
        .chain(&accept.funding_inputs)
        .map(|input| input.input_serial_id)
        .collect::<Vec<_>>();
    serial_ids.sort_unstable();
    if serial_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ContractError::InvalidFundingInput(
            "funding input serial ids are not unique".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn decode_previous_transaction(
    input: &FundingInput,
) -> Result<Transaction, ContractError> {
    Transaction::consensus_decode(&mut input.prev_tx.as_slice()).map_err(|e| {
        ContractError::InvalidFundingInput(format!(
            "could not decode the previous transaction of funding input serial id {}: {e}",
            input.input_serial_id
        ))
    })
}

/// Computes the contract id from the funding transaction and temporary contract id.
pub(crate) fn contract_id_from_transactions(
    transactions: &DlcTransactions,
    temporary_contract_id: &[u8; 32],
) -> [u8; 32] {
    let fund_txid = transactions.fund.compute_txid();
    let fund_output_index = transactions.get_fund_output_index() as u16;
    let mut contract_id = [0; 32];
    for i in 0..32 {
        contract_id[i] = fund_txid[31 - i] ^ temporary_contract_id[i];
    }
    contract_id[30] ^= ((fund_output_index >> 8) & 0xff) as u8;
    contract_id[31] ^= (fund_output_index & 0xff) as u8;
    contract_id
}

//! # This module contains static functions to update the state of a DLC.

use std::ops::Deref;

use bitcoin::hex::DisplayHex;
use bitcoin::psbt::Psbt;
use bitcoin::Amount;
use bitcoin::{consensus::Decodable, Script, Transaction, Witness};
use ddk_dlc::dlc_input::DlcInputInfo;
use ddk_dlc::{DlcTransactions, PartyParams};
use ddk_messages::{
    oracle_msgs::{OracleAnnouncement, OracleAttestation},
    AcceptDlc, FundingSignature, FundingSignatures, OfferDlc, SignDlc, WitnessElement,
};
use ddk_messages::{CloseDlc, FundingInput};
use lightning::util::logger::Logger;
use lightning::{log_debug, log_info};
use secp256k1_zkp::{
    ecdsa::Signature, All, EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey, Signing,
};

use crate::dlc_input::{get_dlc_inputs_from_funding_inputs, get_signature_for_dlc_input};
use crate::Storage;
use crate::{
    contract::{
        accepted_contract::AcceptedContract, contract_info::ContractInfo,
        contract_input::ContractInput, offered_contract::OfferedContract,
        signed_contract::SignedContract, AdaptorInfo,
    },
    conversion_utils::get_tx_input_infos,
    error::Error,
    Blockchain, ChannelId, ContractSigner, ContractSignerProvider, Time, Wallet,
};

/// Creates an [`OfferedContract`] and [`OfferDlc`] message from the provided
/// contract and oracle information.
#[allow(clippy::too_many_arguments)]
pub async fn offer_contract<
    W: Deref,
    B: Deref,
    T: Deref,
    X: ContractSigner,
    SP: Deref,
    C: Signing,
    L: Deref,
>(
    secp: &Secp256k1<C>,
    contract_input: &ContractInput,
    oracle_announcements: Vec<Vec<OracleAnnouncement>>,
    dlc_inputs: Vec<DlcInputInfo>,
    refund_delay: u32,
    counter_party: &PublicKey,
    wallet: &W,
    blockchain: &B,
    time: &T,
    signer_provider: &SP,
    logger: &L,
) -> Result<(OfferedContract, OfferDlc), Error>
where
    W::Target: Wallet,
    B::Target: Blockchain,
    T::Target: Time,
    SP::Target: ContractSignerProvider<Signer = X>,
    L::Target: Logger,
{
    contract_input.validate()?;

    let id = crate::utils::get_new_temporary_id();
    let keys_id = signer_provider.derive_signer_key_id(true, id);
    let signer = signer_provider.derive_contract_signer(keys_id)?;
    let total_collateral = contract_input.offer_collateral + contract_input.accept_collateral;
    let (party_params, funding_inputs_info) = crate::utils::get_party_params(
        secp,
        contract_input.offer_collateral,
        total_collateral,
        dlc_inputs,
        contract_input.fee_rate,
        wallet,
        &signer,
        blockchain,
    )
    .await?;

    log_debug!(
        logger,
        "Created offer contract with offer party params. temp_id={} counter_party={} fund_pubkey={} change_spk={} payout_spk={} dlc_inputs={} input_amount={} collateral={}",
        id.to_lower_hex_string(),
        counter_party.to_string(),
        party_params.fund_pubkey.to_string(),
        party_params.change_script_pubkey.to_string(),
        party_params.payout_script_pubkey.to_string(),
        !party_params.dlc_inputs.is_empty(),
        party_params.input_amount.to_sat(),
        party_params.collateral.to_sat(),
    );

    let offered_contract = OfferedContract::new(
        id,
        contract_input,
        oracle_announcements,
        &party_params,
        &funding_inputs_info,
        counter_party,
        refund_delay,
        time.unix_time_now() as u32,
        keys_id,
    );

    let offer_msg: OfferDlc = (&offered_contract).into();

    Ok((offered_contract, offer_msg))
}

/// Creates an [`AcceptedContract`] and produces
/// the accepting party's cet adaptor signatures.
pub async fn accept_contract<W: Deref, X: ContractSigner, SP: Deref, B: Deref, L: Deref>(
    secp: &Secp256k1<All>,
    offered_contract: &OfferedContract,
    wallet: &W,
    signer_provider: &SP,
    blockchain: &B,
    logger: &L,
) -> Result<(AcceptedContract, AcceptDlc), Error>
where
    W::Target: Wallet,
    B::Target: Blockchain,
    SP::Target: ContractSignerProvider<Signer = X>,
    L::Target: Logger,
{
    let total_collateral = offered_contract.total_collateral;

    let signer = signer_provider.derive_contract_signer(offered_contract.keys_id)?;
    let (accept_params, funding_inputs) = crate::utils::get_party_params(
        secp,
        total_collateral - offered_contract.offer_params.collateral,
        total_collateral,
        // The accept party does not have any DLC inputs, so we pass an empty vector.
        vec![],
        offered_contract.fee_rate_per_vb,
        wallet,
        &signer,
        blockchain,
    )
    .await?;

    log_debug!(
        logger,
        "Created accept party params. temp_id={} counter_party={} fund_pubkey={} change_spk={} payout_spk={} dlc_inputs={} input_amount={} collateral={}",
        offered_contract.id.to_lower_hex_string(),
        offered_contract.counter_party.to_string(),
        accept_params.fund_pubkey.to_string(),
        accept_params.change_script_pubkey.to_string(),
        accept_params.payout_script_pubkey.to_string(),
        !accept_params.dlc_inputs.is_empty(),
        accept_params.input_amount.to_sat(),
        accept_params.collateral.to_sat(),
    );

    // Check BOTH parties for DLC inputs - either party having DLC inputs means we need splicing
    let has_dlc_inputs = !accept_params.dlc_inputs.is_empty()
        || !offered_contract.offer_params.dlc_inputs.is_empty();

    let dlc_transactions = if has_dlc_inputs {
        log_debug!(
            logger,
            "Creating spliced DLC transactions. num_dlc_inputs={}",
            accept_params.dlc_inputs.len() + offered_contract.offer_params.dlc_inputs.len()
        );
        ddk_dlc::create_spliced_dlc_transactions(
            &offered_contract.offer_params,
            &accept_params,
            &offered_contract.contract_info[0].get_payouts(total_collateral)?,
            offered_contract.refund_locktime,
            offered_contract.fee_rate_per_vb,
            0,
            offered_contract.cet_locktime,
            offered_contract.fund_output_serial_id,
        )?
    } else {
        log_debug!(logger, "Creating DLC transactions without splicing.");
        ddk_dlc::create_dlc_transactions(
            &offered_contract.offer_params,
            &accept_params,
            &offered_contract.contract_info[0].get_payouts(total_collateral)?,
            offered_contract.refund_locktime,
            offered_contract.fee_rate_per_vb,
            0,
            offered_contract.cet_locktime,
            offered_contract.fund_output_serial_id,
        )?
    };

    log_info!(
        logger,
        "Created DLC transactions. temp_id={} fund_txid={} funding_spk={} fund_output_value={} refund_txid={} num_cets={}",
        offered_contract.id.to_lower_hex_string(),
        dlc_transactions.fund.compute_txid().to_string(),
        dlc_transactions.funding_script_pubkey.to_string(),
        dlc_transactions.get_fund_output().value.to_sat(),
        dlc_transactions.refund.compute_txid().to_string(),
        dlc_transactions.cets.len()
    );

    let fund_output_value = dlc_transactions.get_fund_output().value;

    let (accepted_contract, adaptor_sigs) = accept_contract_internal(
        secp,
        offered_contract,
        &accept_params,
        &funding_inputs,
        &signer.get_secret_key()?,
        fund_output_value,
        None,
        &dlc_transactions,
    )?;

    log_info!(
        logger,
        "Created accept contract. temp_id={} contract_id={}",
        offered_contract.id.to_lower_hex_string(),
        accepted_contract.get_contract_id().to_lower_hex_string()
    );

    let accept_msg: AcceptDlc = accepted_contract.get_accept_contract_msg(&adaptor_sigs);

    Ok((accepted_contract, accept_msg))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn accept_contract_internal(
    secp: &Secp256k1<All>,
    offered_contract: &OfferedContract,
    accept_params: &PartyParams,
    funding_inputs: &[FundingInput],
    adaptor_secret_key: &SecretKey,
    input_value: Amount,
    input_script_pubkey: Option<&Script>,
    dlc_transactions: &DlcTransactions,
) -> Result<(AcceptedContract, Vec<EcdsaAdaptorSignature>), crate::Error> {
    let total_collateral = offered_contract.total_collateral;

    let input_script_pubkey =
        input_script_pubkey.unwrap_or_else(|| &dlc_transactions.funding_script_pubkey);

    let cet_input = dlc_transactions.cets[0].input[0].clone();

    let (adaptor_info, adaptor_sig) = offered_contract.contract_info[0].get_adaptor_info(
        secp,
        offered_contract.total_collateral,
        adaptor_secret_key,
        input_script_pubkey,
        input_value,
        &dlc_transactions.cets,
        0,
    )?;
    let mut adaptor_infos = vec![adaptor_info];
    let mut adaptor_sigs = adaptor_sig;

    let DlcTransactions {
        fund,
        cets,
        refund,
        funding_script_pubkey,
        pending_close_txs: _,
    } = dlc_transactions;

    let mut cets = cets.clone();

    for contract_info in offered_contract.contract_info.iter().skip(1) {
        let payouts = contract_info.get_payouts(total_collateral)?;

        let tmp_cets = ddk_dlc::create_cets(
            &cet_input,
            &offered_contract.offer_params.payout_script_pubkey,
            offered_contract.offer_params.payout_serial_id,
            &accept_params.payout_script_pubkey,
            accept_params.payout_serial_id,
            &payouts,
            0,
        );

        let (adaptor_info, adaptor_sig) = contract_info.get_adaptor_info(
            secp,
            offered_contract.total_collateral,
            adaptor_secret_key,
            input_script_pubkey,
            input_value,
            &tmp_cets,
            adaptor_sigs.len(),
        )?;

        cets.extend(tmp_cets);

        adaptor_infos.push(adaptor_info);
        adaptor_sigs.extend(adaptor_sig);
    }

    let refund_signature = ddk_dlc::util::get_raw_sig_for_tx_input(
        secp,
        refund,
        0,
        input_script_pubkey,
        input_value,
        adaptor_secret_key,
    )?;

    let dlc_transactions = DlcTransactions {
        fund: fund.clone(),
        cets,
        refund: refund.clone(),
        funding_script_pubkey: funding_script_pubkey.clone(),
        pending_close_txs: vec![],
    };

    let accepted_contract = AcceptedContract {
        offered_contract: offered_contract.clone(),
        adaptor_infos,
        // Drop own adaptor signatures as no point keeping them.
        adaptor_signatures: None,
        accept_params: accept_params.clone(),
        funding_inputs: funding_inputs.to_vec(),
        dlc_transactions,
        accept_refund_signature: refund_signature,
    };

    Ok((accepted_contract, adaptor_sigs))
}

/// Verifies the information of the accepting party [`Accept` message](dlc_messages::AcceptDlc),
/// creates a [`SignedContract`], and generates the offering party CET adaptor signatures.
pub async fn verify_accepted_and_sign_contract<
    W: Deref,
    X: ContractSigner,
    SP: Deref,
    S: Deref,
    L: Deref,
>(
    secp: &Secp256k1<All>,
    offered_contract: &OfferedContract,
    accept_msg: &AcceptDlc,
    wallet: &W,
    signer_provider: &SP,
    storage: &S,
    logger: &L,
) -> Result<(SignedContract, SignDlc), Error>
where
    W::Target: Wallet,
    SP::Target: ContractSignerProvider<Signer = X>,
    S::Target: Storage,
    L::Target: Logger,
{
    let (tx_input_infos, input_amount) = get_tx_input_infos(&accept_msg.funding_inputs)?;
    let accept_dlc_inputs = get_dlc_inputs_from_funding_inputs(&accept_msg.funding_inputs);

    let accept_params = PartyParams {
        fund_pubkey: accept_msg.funding_pubkey,
        change_script_pubkey: accept_msg.change_spk.clone(),
        change_serial_id: accept_msg.change_serial_id,
        payout_script_pubkey: accept_msg.payout_spk.clone(),
        payout_serial_id: accept_msg.payout_serial_id,
        inputs: tx_input_infos,
        dlc_inputs: accept_dlc_inputs.clone(),
        input_amount,
        collateral: accept_msg.accept_collateral,
    };

    log_debug!(
        logger,
        "Retrieved the party params for the accepting party. temp_id={} counter_party={} fund_pubkey={} change_spk={} payout_spk={} dlc_inputs={} input_amount={} collateral={}",
        offered_contract.id.to_lower_hex_string(),
        offered_contract.counter_party.to_string(),
        accept_params.fund_pubkey.to_string(),
        accept_params.change_script_pubkey.to_string(),
        accept_params.payout_script_pubkey.to_string(),
        !accept_params.dlc_inputs.is_empty(),
        accept_params.input_amount.to_sat(),
        accept_params.collateral.to_sat(),
    );

    let cet_adaptor_signatures = accept_msg
        .cet_adaptor_signatures
        .ecdsa_adaptor_signatures
        .iter()
        .map(|x| x.signature)
        .collect::<Vec<_>>();

    let total_collateral = offered_contract.total_collateral;

    // Check BOTH parties for DLC inputs - either party having DLC inputs means we need splicing
    let has_dlc_inputs =
        !accept_dlc_inputs.is_empty() || !offered_contract.offer_params.dlc_inputs.is_empty();

    let dlc_transactions = if has_dlc_inputs {
        log_debug!(
            logger,
            "Creating spliced DLC transactions. num_dlc_inputs={}",
            accept_dlc_inputs.len() + offered_contract.offer_params.dlc_inputs.len()
        );
        ddk_dlc::create_spliced_dlc_transactions(
            &offered_contract.offer_params,
            &accept_params,
            &offered_contract.contract_info[0].get_payouts(total_collateral)?,
            offered_contract.refund_locktime,
            offered_contract.fee_rate_per_vb,
            0,
            offered_contract.cet_locktime,
            offered_contract.fund_output_serial_id,
        )?
    } else {
        log_debug!(logger, "Creating DLC transactions without splicing.");
        ddk_dlc::create_dlc_transactions(
            &offered_contract.offer_params,
            &accept_params,
            &offered_contract.contract_info[0].get_payouts(total_collateral)?,
            offered_contract.refund_locktime,
            offered_contract.fee_rate_per_vb,
            0,
            offered_contract.cet_locktime,
            offered_contract.fund_output_serial_id,
        )?
    };

    log_info!(
        logger,
        "Created DLC transactions. temp_id={} fund_txid={} funding_spk={} fund_output_value={} refund_txid={} num_cets={}",
        offered_contract.id.to_lower_hex_string(),
        dlc_transactions.fund.compute_txid().to_string(),
        dlc_transactions.funding_script_pubkey.to_string(),
        dlc_transactions.get_fund_output().value.to_sat(),
        dlc_transactions.refund.compute_txid().to_string(),
        dlc_transactions.cets.len()
    );

    let fund_output_value = dlc_transactions.get_fund_output().value;

    let signer = signer_provider.derive_contract_signer(offered_contract.keys_id)?;

    let (signed_contract, adaptor_sigs) = verify_accepted_and_sign_contract_internal(
        secp,
        offered_contract,
        &accept_params,
        &accept_msg.funding_inputs,
        &accept_msg.refund_signature,
        &cet_adaptor_signatures,
        fund_output_value,
        wallet,
        &signer,
        None,
        None,
        &dlc_transactions,
        None,
        storage,
        signer_provider,
        logger,
    )
    .await?;

    let contract_id = signed_contract.accepted_contract.get_contract_id_string();

    log_info!(
        logger,
        "Signed and verified accept message. tmp_id={} contract_id={} num_adaptor_sigs={}",
        offered_contract.id.to_lower_hex_string(),
        contract_id,
        adaptor_sigs.len()
    );

    let signed_msg: SignDlc = signed_contract.get_sign_dlc(adaptor_sigs);

    Ok((signed_contract, signed_msg))
}

fn populate_psbt(psbt: &mut Psbt, all_funding_inputs: &[&FundingInput]) -> Result<(), Error> {
    // add witness utxo to fund_psbt for all inputs
    for (input_index, x) in all_funding_inputs.iter().enumerate() {
        let tx = Transaction::consensus_decode(&mut x.prev_tx.as_slice()).map_err(|_| {
            Error::InvalidParameters(
                "Could not decode funding input previous tx parameter".to_string(),
            )
        })?;
        let vout = x.prev_tx_vout;
        let tx_out = tx.output.get(vout as usize).ok_or_else(|| {
            Error::InvalidParameters(format!("Previous tx output not found at index {vout}"))
        })?;

        psbt.inputs[input_index].witness_utxo = Some(tx_out.clone());
        psbt.inputs[input_index].redeem_script = Some(x.redeem_script.clone());
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_accepted_and_sign_contract_internal<
    W: Deref,
    X: ContractSigner,
    S: Deref,
    SP: Deref,
    L: Deref,
>(
    secp: &Secp256k1<All>,
    offered_contract: &OfferedContract,
    accept_params: &PartyParams,
    funding_inputs_info: &[FundingInput],
    refund_signature: &Signature,
    cet_adaptor_signatures: &[EcdsaAdaptorSignature],
    input_value: Amount,
    wallet: &W,
    signer: &X,
    input_script_pubkey: Option<&Script>,
    counter_adaptor_pk: Option<PublicKey>,
    dlc_transactions: &DlcTransactions,
    channel_id: Option<ChannelId>,
    storage: &S,
    signer_provider: &SP,
    logger: &L,
) -> Result<(SignedContract, Vec<EcdsaAdaptorSignature>), Error>
where
    W::Target: Wallet,
    S::Target: Storage,
    SP::Target: ContractSignerProvider<Signer = X>,
    L::Target: Logger,
{
    let DlcTransactions {
        fund,
        cets,
        refund,
        funding_script_pubkey,
        pending_close_txs: _,
    } = dlc_transactions;

    let mut fund_psbt = Psbt::from_unsigned_tx(fund.clone())
        .map_err(|_| Error::InvalidState("Tried to create PSBT from signed tx".to_string()))?;
    let mut cets = cets.clone();

    let input_script_pubkey = input_script_pubkey.unwrap_or_else(|| funding_script_pubkey);
    let counter_adaptor_pk = counter_adaptor_pk.unwrap_or(accept_params.fund_pubkey);

    ddk_dlc::verify_tx_input_sig(
        secp,
        refund_signature,
        refund,
        0,
        input_script_pubkey,
        input_value,
        &counter_adaptor_pk,
    )?;

    log_debug!(
        logger,
        "Verified refund signature. temp_id={} refund_txid={}",
        offered_contract.id.to_lower_hex_string(),
        refund.compute_txid().to_string(),
    );

    let (adaptor_info, mut adaptor_index) = offered_contract.contract_info[0]
        .verify_and_get_adaptor_info(
            secp,
            offered_contract.total_collateral,
            &counter_adaptor_pk,
            input_script_pubkey,
            input_value,
            &cets,
            cet_adaptor_signatures,
            0,
        )?;

    let mut adaptor_infos = vec![adaptor_info];

    let cet_input = cets[0].input[0].clone();

    let total_collateral = offered_contract.offer_params.collateral + accept_params.collateral;

    for contract_info in offered_contract.contract_info.iter().skip(1) {
        let payouts = contract_info.get_payouts(total_collateral)?;

        let tmp_cets = ddk_dlc::create_cets(
            &cet_input,
            &offered_contract.offer_params.payout_script_pubkey,
            offered_contract.offer_params.payout_serial_id,
            &accept_params.payout_script_pubkey,
            accept_params.payout_serial_id,
            &payouts,
            0,
        );

        let (adaptor_info, tmp_adaptor_index) = contract_info.verify_and_get_adaptor_info(
            secp,
            offered_contract.total_collateral,
            &accept_params.fund_pubkey,
            funding_script_pubkey,
            input_value,
            &tmp_cets,
            cet_adaptor_signatures,
            adaptor_index,
        )?;

        adaptor_index = tmp_adaptor_index;

        cets.extend(tmp_cets);

        adaptor_infos.push(adaptor_info);
    }

    let mut own_signatures: Vec<EcdsaAdaptorSignature> = Vec::new();

    for (contract_info, adaptor_info) in offered_contract
        .contract_info
        .iter()
        .zip(adaptor_infos.iter())
    {
        let sigs = contract_info.get_adaptor_signatures(
            secp,
            adaptor_info,
            &signer,
            input_script_pubkey,
            input_value,
            &cets,
        )?;
        own_signatures.extend(sigs);
    }

    // get all funding inputs
    let mut all_funding_inputs = offered_contract
        .funding_inputs
        .iter()
        .chain(funding_inputs_info.iter())
        .collect::<Vec<_>>();
    // sort by serial id
    all_funding_inputs.sort_by_key(|x| x.input_serial_id);

    log_info!(
        logger,
        "Populating PSBT for signing funding inputs. num_funding_inputs={}",
        all_funding_inputs.len(),
    );
    populate_psbt(&mut fund_psbt, &all_funding_inputs)?;

    let mut witnesses: Vec<Witness> = Vec::new();
    for funding_input in offered_contract.funding_inputs.iter() {
        let input_index = all_funding_inputs
            .iter()
            .position(|y| y == &funding_input)
            .ok_or_else(|| {
                Error::InvalidState(format!(
                    "Could not find input for serial id {}",
                    funding_input.input_serial_id
                ))
            })?;

        // If the funding input contains DLC information, we can sign the input internally using the
        // corresponding contract id to get the keys id.
        //
        // The funding pubkeys for both the offer and accepting party are deterministically derived from
        // the ContractSignerProvider. Becuase of this, we do not need to prompt the consumer for the signature.
        if let Some(dlc_input) = &funding_input.dlc_input {
            log_debug!(
                logger,
                "Signing DLC input. temp_id={} splice_contract_id={} local_fund_pubkey={} remote_fund_pubkey={} input_index={}",
                offered_contract.id.to_lower_hex_string(),
                dlc_input.contract_id.to_lower_hex_string(),
                dlc_input.local_fund_pubkey.to_string(),
                dlc_input.remote_fund_pubkey.to_string(),
                input_index,
            );
            let dlc_input_signature = get_signature_for_dlc_input(
                secp,
                funding_input,
                fund,
                input_index,
                &dlc_input.contract_id,
                storage,
                signer_provider,
            )
            .await?;

            let witness = Witness::from_slice(&[dlc_input_signature]);

            witnesses.push(witness);
            continue;
        }

        wallet.sign_psbt_input(&mut fund_psbt, input_index).await?;

        let witness = fund_psbt.inputs[input_index]
            .final_script_witness
            .clone()
            .ok_or(Error::InvalidParameters(
                "No witness from signing psbt input".to_string(),
            ))?;

        witnesses.push(witness);
    }

    log_debug!(
        logger,
        "Signed funding inputs. temp_id={} num_signatures={}",
        offered_contract.id.to_lower_hex_string(),
        witnesses.len()
    );

    let funding_signatures: Vec<FundingSignature> = witnesses
        .into_iter()
        .map(|witness| {
            let witness_elements = witness
                .iter()
                .map(|z| WitnessElement {
                    witness: z.to_vec(),
                })
                .collect();
            Ok(FundingSignature { witness_elements })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let offer_refund_signature = ddk_dlc::util::get_raw_sig_for_tx_input(
        secp,
        refund,
        0,
        input_script_pubkey,
        input_value,
        &signer.get_secret_key()?,
    )?;

    let dlc_transactions = DlcTransactions {
        fund: fund.clone(),
        cets,
        refund: refund.clone(),
        funding_script_pubkey: funding_script_pubkey.clone(),
        pending_close_txs: vec![],
    };

    let accepted_contract = AcceptedContract {
        offered_contract: offered_contract.clone(),
        accept_params: accept_params.clone(),
        funding_inputs: funding_inputs_info.to_vec(),
        adaptor_infos,
        adaptor_signatures: Some(cet_adaptor_signatures.to_vec()),
        accept_refund_signature: *refund_signature,
        dlc_transactions,
    };

    let signed_contract = SignedContract {
        accepted_contract,
        adaptor_signatures: None,
        offer_refund_signature,
        funding_signatures: FundingSignatures { funding_signatures },
        channel_id,
    };

    Ok((signed_contract, own_signatures))
}

/// Verifies the information from the offer party [`Sign` message](dlc_messages::SignDlc),
/// creates the accepting party's [`SignedContract`] and returns it along with the
/// signed fund transaction.
pub async fn verify_signed_contract<W: Deref, S: Deref, SP: Deref, X: ContractSigner, L: Deref>(
    secp: &Secp256k1<All>,
    accepted_contract: &AcceptedContract,
    sign_msg: &SignDlc,
    wallet: &W,
    storage: &S,
    signer_provider: &SP,
    logger: &L,
) -> Result<(SignedContract, Transaction), Error>
where
    W::Target: Wallet,
    S::Target: Storage,
    SP::Target: ContractSignerProvider<Signer = X>,
    L::Target: Logger,
{
    let cet_adaptor_signatures: Vec<_> = (&sign_msg.cet_adaptor_signatures).into();
    verify_signed_contract_internal(
        secp,
        accepted_contract,
        &sign_msg.refund_signature,
        &cet_adaptor_signatures,
        &sign_msg.funding_signatures,
        accepted_contract.dlc_transactions.get_fund_output().value,
        None,
        None,
        wallet,
        None,
        storage,
        signer_provider,
        logger,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_signed_contract_internal<
    W: Deref,
    SP: Deref,
    S: Deref,
    X: ContractSigner,
    L: Deref,
>(
    secp: &Secp256k1<All>,
    accepted_contract: &AcceptedContract,
    refund_signature: &Signature,
    cet_adaptor_signatures: &[EcdsaAdaptorSignature],
    funding_signatures: &FundingSignatures,
    input_value: Amount,
    input_script_pubkey: Option<&Script>,
    counter_adaptor_pk: Option<PublicKey>,
    wallet: &W,
    channel_id: Option<ChannelId>,
    storage: &S,
    signer_provider: &SP,
    logger: &L,
) -> Result<(SignedContract, Transaction), Error>
where
    W::Target: Wallet,
    S::Target: Storage,
    SP::Target: ContractSignerProvider<Signer = X>,
    L::Target: Logger,
{
    let offered_contract = &accepted_contract.offered_contract;
    let input_script_pubkey = input_script_pubkey
        .unwrap_or_else(|| &accepted_contract.dlc_transactions.funding_script_pubkey);
    let counter_adaptor_pk =
        counter_adaptor_pk.unwrap_or(accepted_contract.offered_contract.offer_params.fund_pubkey);

    ddk_dlc::verify_tx_input_sig(
        secp,
        refund_signature,
        &accepted_contract.dlc_transactions.refund,
        0,
        input_script_pubkey,
        input_value,
        &counter_adaptor_pk,
    )?;

    log_debug!(
        logger,
        "Verified refund signature. contract_id={} refund_txid={}",
        accepted_contract.get_contract_id_string(),
        accepted_contract
            .dlc_transactions
            .refund
            .compute_txid()
            .to_string(),
    );
    let mut adaptor_sig_start = 0;

    for (adaptor_info, contract_info) in accepted_contract
        .adaptor_infos
        .iter()
        .zip(offered_contract.contract_info.iter())
    {
        adaptor_sig_start = contract_info.verify_adaptor_info(
            secp,
            &counter_adaptor_pk,
            input_script_pubkey,
            input_value,
            &accepted_contract.dlc_transactions.cets,
            cet_adaptor_signatures,
            adaptor_sig_start,
            adaptor_info,
        )?;
    }

    log_debug!(
        logger,
        "Verified adaptor signatures. contract_id={} num_adaptor_infos={}",
        accepted_contract.get_contract_id_string(),
        accepted_contract.adaptor_infos.len(),
    );

    let fund_tx = &accepted_contract.dlc_transactions.fund;
    let mut fund_psbt = Psbt::from_unsigned_tx(fund_tx.clone())
        .map_err(|_| Error::InvalidState("Tried to create PSBT from signed tx".to_string()))?;

    // get all funding inputs
    let mut all_funding_inputs = offered_contract
        .funding_inputs
        .iter()
        .chain(accepted_contract.funding_inputs.iter())
        .collect::<Vec<_>>();
    // sort by serial id
    all_funding_inputs.sort_by_key(|x| x.input_serial_id);

    log_debug!(
        logger,
        "Populating PSBT for signing funding inputs. contract_id={} num_funding_inputs={}",
        accepted_contract.get_contract_id_string(),
        all_funding_inputs.len(),
    );
    populate_psbt(&mut fund_psbt, &all_funding_inputs)?;

    for (funding_input, funding_signatures) in offered_contract
        .funding_inputs
        .iter()
        .zip(funding_signatures.funding_signatures.iter())
    {
        let input_index = all_funding_inputs
            .iter()
            .position(|x| x == &funding_input)
            .ok_or_else(|| {
                Error::InvalidState(format!(
                    "Could not find input for serial id {}",
                    funding_input.input_serial_id
                ))
            })?;

        // If the funding input contains DLC information, we know that the corresponding funding signature
        // from the offer party is their half of the DLC input and we can build the valid redeem script.
        if let Some(dlc_input) = &funding_input.dlc_input {
            let dlc_input_info: DlcInputInfo = funding_input.into();
            log_debug!(
                logger,
                "Verifying DLC input signature. contract_id={} input_index={} remote_fund_pubkey={} local_fund_pubkey={}",
                accepted_contract.get_contract_id_string(),
                input_index,
                dlc_input.remote_fund_pubkey.to_string(),
                dlc_input.local_fund_pubkey.to_string(),
            );

            // Verify the signature from the offer party is valid for the DLC input.
            ddk_dlc::dlc_input::verify_dlc_funding_input_signature(
                secp,
                fund_tx,
                input_index,
                &dlc_input_info,
                funding_signatures.witness_elements[0].witness.clone(),
                &dlc_input.local_fund_pubkey,
            )?;

            log_debug!(
                logger,
                "Signing DLC input. contract_id={} input_index={}",
                accepted_contract.get_contract_id_string(),
                input_index,
            );
            // Get the signature for the DLC input from the accepting party.
            let my_dlc_input_signature = get_signature_for_dlc_input(
                secp,
                funding_input,
                fund_tx,
                input_index,
                &dlc_input.contract_id,
                storage,
                signer_provider,
            )
            .await?;

            // Build the redeem script for the DLC input.
            let completed_witness = ddk_dlc::dlc_input::combine_dlc_input_signatures(
                &dlc_input_info,
                &my_dlc_input_signature,
                &funding_signatures.witness_elements[0].witness,
                &dlc_input.remote_fund_pubkey,
                &dlc_input.local_fund_pubkey,
            );

            log_debug!(
                logger,
                "Completed the signatures for the DLC input. contract_id={} input_index={}",
                accepted_contract.get_contract_id_string(),
                input_index,
            );

            fund_psbt.inputs[input_index].final_script_witness = Some(completed_witness);
        } else {
            fund_psbt.inputs[input_index].final_script_witness = Some(Witness::from_slice(
                &funding_signatures
                    .witness_elements
                    .iter()
                    .map(|x| x.witness.clone())
                    .collect::<Vec<_>>(),
            ));
        }
    }

    for funding_input in &accepted_contract.funding_inputs {
        let input_index = all_funding_inputs
            .iter()
            .position(|x| x == &funding_input)
            .ok_or_else(|| {
                Error::InvalidState(format!(
                    "Could not find input for serial id {}",
                    funding_input.input_serial_id
                ))
            })?;

        log_debug!(
            logger,
            "Signing funding input. contract_id={} input_index={}",
            accepted_contract.get_contract_id_string(),
            input_index,
        );
        wallet.sign_psbt_input(&mut fund_psbt, input_index).await?;
    }

    let signed_contract = SignedContract {
        accepted_contract: accepted_contract.clone(),
        adaptor_signatures: Some(cet_adaptor_signatures.to_vec()),
        offer_refund_signature: *refund_signature,
        funding_signatures: funding_signatures.clone(),
        channel_id,
    };

    let transaction = fund_psbt.extract_tx_unchecked_fee_rate();

    Ok((signed_contract, transaction))
}

/// Signs and return the CET that can be used to close the given contract.
pub fn get_signed_cet<C: Signing, S: Deref, L: Deref>(
    secp: &Secp256k1<C>,
    contract: &SignedContract,
    contract_info: &ContractInfo,
    adaptor_info: &AdaptorInfo,
    attestations: &[(usize, OracleAttestation)],
    signer: S,
    logger: &L,
) -> Result<Transaction, Error>
where
    S::Target: ContractSigner,
    L::Target: Logger,
{
    let contract_id = contract.accepted_contract.get_contract_id_string();
    log_info!(
        logger,
        "Getting the signed CET for the Oracle Attestation. contract_id={} outcomes={:?} event_id={}",
        contract_id,
        attestations
            .iter()
            .map(|(_, a)| &a.outcomes)
            .collect::<Vec<_>>(),
        attestations.first().unwrap().1.event_id,
    );
    let (range_info, sigs) =
        crate::utils::get_range_info_and_oracle_sigs(contract_info, adaptor_info, attestations)?;
    let mut cet = contract.accepted_contract.dlc_transactions.cets[range_info.cet_index].clone();
    let offered_contract = &contract.accepted_contract.offered_contract;

    let (adaptor_sigs, other_pubkey) = if offered_contract.is_offer_party {
        (
            contract
                .accepted_contract
                .adaptor_signatures
                .as_ref()
                .unwrap(),
            &contract.accepted_contract.accept_params.fund_pubkey,
        )
    } else {
        (
            contract.adaptor_signatures.as_ref().unwrap(),
            &offered_contract.offer_params.fund_pubkey,
        )
    };

    let funding_sk = signer.get_secret_key()?;

    ddk_dlc::sign_cet(
        secp,
        &mut cet,
        &adaptor_sigs[range_info.adaptor_index],
        &sigs,
        &funding_sk,
        other_pubkey,
        &contract
            .accepted_contract
            .dlc_transactions
            .funding_script_pubkey,
        contract
            .accepted_contract
            .dlc_transactions
            .get_fund_output()
            .value,
    )?;

    Ok(cet)
}

/// Signs and return the refund transaction to refund the contract.
pub fn get_signed_refund<C: Signing, S: Deref, L: Deref>(
    secp: &Secp256k1<C>,
    contract: &SignedContract,
    signer: S,
    logger: &L,
) -> Result<Transaction, Error>
where
    S::Target: ContractSigner,
    L::Target: Logger,
{
    log_info!(
        logger,
        "Getting signed refund transaction. contract_id={}",
        contract.accepted_contract.get_contract_id_string()
    );
    let accepted_contract = &contract.accepted_contract;
    let offered_contract = &accepted_contract.offered_contract;
    let funding_script_pubkey = &accepted_contract.dlc_transactions.funding_script_pubkey;
    let fund_output_value = accepted_contract.dlc_transactions.get_fund_output().value;
    let (other_fund_pubkey, other_sig) = if offered_contract.is_offer_party {
        (
            &accepted_contract.accept_params.fund_pubkey,
            &accepted_contract.accept_refund_signature,
        )
    } else {
        (
            &offered_contract.offer_params.fund_pubkey,
            &contract.offer_refund_signature,
        )
    };

    let fund_priv_key = signer.get_secret_key()?;
    let mut refund = accepted_contract.dlc_transactions.refund.clone();
    ddk_dlc::util::sign_multi_sig_input(
        secp,
        &mut refund,
        other_sig,
        other_fund_pubkey,
        &fund_priv_key,
        funding_script_pubkey,
        fund_output_value,
        0,
    )?;
    Ok(refund)
}

/// Creates a cooperative close transaction and signs it with the local party's key.
pub fn create_cooperative_close<C: Signing, SP: Deref, L: Deref>(
    secp: &Secp256k1<C>,
    signed_contract: &SignedContract,
    counter_payout: Amount,
    signer_provider: &SP,
    logger: &L,
) -> Result<(CloseDlc, Transaction), Error>
where
    SP::Target: ContractSignerProvider,
    L::Target: Logger,
{
    let accepted_contract = &signed_contract.accepted_contract;
    let offered_contract = &accepted_contract.offered_contract;
    let total_collateral = offered_contract.total_collateral;

    if counter_payout > total_collateral {
        return Err(Error::InvalidParameters(
            "Counter payout is greater than total collateral".to_string(),
        ));
    }

    let offer_payout = total_collateral - counter_payout;
    let fund_output_value = accepted_contract.dlc_transactions.get_fund_output().value;
    let fund_outpoint = accepted_contract.dlc_transactions.get_fund_outpoint();

    // Create the cooperative close transaction
    let close_tx = ddk_dlc::channel::create_collaborative_close_transaction(
        &offered_contract.offer_params,
        offer_payout,
        &accepted_contract.accept_params,
        counter_payout,
        fund_outpoint,
        fund_output_value,
        &[], // TODO: Add additional inputs parameter to prevent free option problem
    );

    log_debug!(
        logger,
        "Created cooperative close transaction. contract_id={} close_txid={} offer_payout={} counter_payout={}",
        accepted_contract.get_contract_id_string(),
        close_tx.compute_txid().to_string(),
        offer_payout,
        counter_payout,
    );

    // Get our private key and sign the transaction
    let signer = signer_provider.derive_contract_signer(offered_contract.keys_id)?;
    let fund_private_key = signer.get_secret_key()?;

    let close_signature = ddk_dlc::util::get_raw_sig_for_tx_input(
        secp,
        &close_tx,
        0,
        &accepted_contract.dlc_transactions.funding_script_pubkey,
        fund_output_value,
        &fund_private_key,
    )?;

    // Create the CloseDlc message
    let close_message = CloseDlc {
        protocol_version: crate::conversion_utils::PROTOCOL_VERSION,
        contract_id: accepted_contract.get_contract_id(),
        close_signature,
        accept_payout: counter_payout,
        fee_rate_per_vb: offered_contract.fee_rate_per_vb,
        fund_input_serial_id: offered_contract.fund_output_serial_id,
        funding_inputs: accepted_contract.funding_inputs.clone(),
        funding_signatures: signed_contract.funding_signatures.clone(),
    };

    Ok((close_message, close_tx))
}

/// Verifies and completes a cooperative close transaction using the counter party's signature.
pub fn complete_cooperative_close<C: Signing, SP: Deref, L: Deref>(
    secp: &Secp256k1<C>,
    signed_contract: &SignedContract,
    close_message: &CloseDlc,
    signer_provider: &SP,
    logger: &L,
) -> Result<Transaction, Error>
where
    SP::Target: ContractSignerProvider,
    L::Target: Logger,
{
    let accepted_contract = &signed_contract.accepted_contract;
    let offered_contract = &accepted_contract.offered_contract;
    let fund_output_value = accepted_contract.dlc_transactions.get_fund_output().value;
    let fund_outpoint = accepted_contract.dlc_transactions.get_fund_outpoint();

    let total_collateral = offered_contract.total_collateral;
    let offer_payout = total_collateral - close_message.accept_payout;

    // Recreate the close transaction to verify
    let mut close_tx = ddk_dlc::channel::create_collaborative_close_transaction(
        &offered_contract.offer_params,
        offer_payout,
        &accepted_contract.accept_params,
        close_message.accept_payout,
        fund_outpoint,
        fund_output_value,
        &[], // No additional inputs for contract cooperative close verification
    );

    log_debug!(
        logger,
        "Recreated close transaction for cooperative close verification. contract_id={} close_txid={} offer_payout={} counter_payout={}",
        accepted_contract.get_contract_id_string(),
        close_tx.compute_txid().to_string(),
        offer_payout,
        close_message.accept_payout,
    );

    // Get our private key
    let signer = signer_provider.derive_contract_signer(offered_contract.keys_id)?;
    let fund_private_key = signer.get_secret_key()?;

    // Get counter party's pubkey
    let counter_pubkey = if offered_contract.is_offer_party {
        &accepted_contract.accept_params.fund_pubkey
    } else {
        &offered_contract.offer_params.fund_pubkey
    };

    // Sign and combine signatures
    ddk_dlc::util::sign_multi_sig_input(
        secp,
        &mut close_tx,
        &close_message.close_signature,
        counter_pubkey,
        &fund_private_key,
        &accepted_contract.dlc_transactions.funding_script_pubkey,
        fund_output_value,
        0,
    )?;

    Ok(close_tx)
}

//! Contract signing by a signer outside this process.
//!
//! [`accept_offer`](super::accept_offer), [`sign_accept_spliced`](super::sign_accept_spliced)
//! and [`finalize_sign_spliced`](super::finalize_sign_spliced) take a party's
//! DLC funding keys in the process. A vault, an HSM, a browser extension or a
//! hardware wallet keeps them somewhere else. This module splits each of those
//! steps around the signer: a *request* says exactly what to sign, the signer
//! answers it, and *completion* verifies the answer and builds the result. The
//! key-in-process functions are the same two halves with a signer in the
//! process between them, so there is one code path whoever holds the keys.
//!
//! | Step | Request | The signer answers with | Completion | Result |
//! |------|---------|-------------------------|------------|--------|
//! | accept an offer | [`accept_request`] | [`ContractSignatures`] | [`complete_accept`] | [`AcceptResult`] |
//! | sign the accept | [`sign_request`] | [`ContractSignatures`] and the signed funding PSBT | [`complete_sign`] | [`SignResult`] |
//! | finalize the funding | [`finalize_request`] | the signed funding PSBT | [`complete_finalize`] | the funding transaction |
//!
//! Each request verifies the counterparty's message first, so a signer is only
//! ever asked to sign for a contract the counterparty has already committed
//! to.
//!
//! # What gets signed
//!
//! The *contract funding key* signs the refund transaction and one adaptor
//! signature per CET outcome ([`ContractSigningRequest`]). Each is a PSBT
//! spending the 2-of-2 funding output, so the signer computes its sighash the
//! way it would for any PSBT; an adaptor signature is then encrypted to its
//! adaptor point instead of being a plain signature.
//!
//! The funding inputs are signed in the funding PSBT ([`FundingSigningRequest`]).
//! Wallet inputs are signed and finalized as through the [`signing`](super::signing)
//! layer. A splice input spends the previous contract's 2-of-2, of which each
//! party holds one key: the signer leaves its half as a partial signature
//! (`partial_sigs`), and the two halves are combined when the funding
//! transaction is finalized.
//!
//! # Nothing the signer returns is trusted
//!
//! Completion rejects a funding PSBT whose unsigned transaction differs from
//! the one requested, a signature by any key but the requested one, a sighash
//! other than `SIGHASH_ALL`, and a number of adaptor signatures other than the
//! number requested. It verifies the refund signature, every CET adaptor
//! signature and every splice signature before it builds anything from them.
//! Wallet input witnesses are taken as finalized, as in the rest of the
//! stateless API.
//!
//! A request can only be made by its request function, and completion trusts
//! what it carries: the transactions and adaptor points the signer was asked
//! about are not rebuilt a second time. Keep the request between the two
//! halves, or make it again from the same messages.

use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::Message;
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::{Amount, Transaction};
use ddk_dlc::secp256k1_zkp::ecdsa::Signature;
use ddk_dlc::secp256k1_zkp::{EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey};
use ddk_dlc::DlcTransactions;
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, FundingSignatures, OfferDlc, SignDlc};

use super::context::{
    apply_funding_signatures, build_context, cet_adaptor_points, context_from_messages,
    contract_id_from_transactions, dlc_party_params, ensure_funding_key, ensure_no_dlc_inputs,
    ensure_sign_message, ensure_unique_input_serial_ids, funding_input_index,
    verify_contract_signatures,
};
use super::create::validate_offer;
use super::error::ContractError;
use super::psbt::{
    build_funding_psbt, ensure_psbt_matches_funding_transaction, extract_funding_signatures,
    splice_signature,
};
use super::splice::{prior_temporary_contract_id, verify_splice_signature};
use super::types::{
    random_serial_id, AcceptOfferParams, AcceptResult, DlcInputSigningKey, Party, PartyParams,
    SignResult,
};

/// What the contract funding key signs: the refund transaction, and one CET
/// adaptor signature per entry of `adaptor_points`.
///
/// Every PSBT spends the contract's 2-of-2 funding output and carries, on its
/// one input, the funding output as `witness_utxo`, the 2-of-2 as
/// `witness_script`, and `SIGHASH_ALL`.
#[derive(Clone, Debug)]
pub struct ContractSigningRequest {
    /// The contract funding key that must sign everything here.
    pub funding_pubkey: PublicKey,
    /// The refund transaction.
    pub refund: Psbt,
    /// Every CET, by CET index.
    pub cets: Vec<Psbt>,
    /// One entry per adaptor signature, in signature order: the index into
    /// `cets` of the CET it signs and the adaptor point it is encrypted to. A
    /// CET appears once per oracle outcome combination that settles to it.
    pub adaptor_points: Vec<(usize, PublicKey)>,
}

/// A signer's answer to a [`ContractSigningRequest`].
#[derive(Clone, Debug)]
pub struct ContractSignatures {
    /// The signature of the refund transaction, with its sighash type, which
    /// must be `SIGHASH_ALL`.
    pub refund_signature: bitcoin::ecdsa::Signature,
    /// One adaptor signature per entry of the request's `adaptor_points`, in
    /// the same order.
    pub cet_adaptor_signatures: Vec<EcdsaAdaptorSignature>,
}

/// The funding PSBT, and the splice inputs of it this party signs half of.
///
/// The signer returns the PSBT with this party's wallet inputs finalized, as
/// through the [`signing`](super::signing) layer, and a `SIGHASH_ALL` partial
/// signature by each `splice_inputs` entry's key on that input. Inputs of the
/// counterparty stay as they are.
#[derive(Clone, Debug)]
pub struct FundingSigningRequest {
    /// The funding PSBT, as [`create_funding_psbt`](super::create_funding_psbt)
    /// builds it.
    pub psbt: Psbt,
    /// This party's half of each splice input, in input order.
    pub splice_inputs: Vec<SpliceInputToSign>,
}

/// A splice input this party signs one half of, with its funding key of the
/// previous contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpliceInputToSign {
    /// The serial id of the splice input.
    pub input_serial_id: u64,
    /// The index of the input in the funding transaction and the funding PSBT.
    pub input_index: usize,
    /// The key that signs: this party's funding key in the previous contract.
    pub funding_pubkey: PublicKey,
    /// The previous contract's temporary id, from which a signer that derives
    /// a key per contract, such as
    /// [`ContractKeyProvider`](super::ContractKeyProvider), derives
    /// `funding_pubkey`'s secret key.
    pub temporary_contract_id: [u8; 32],
}

/// What the accepting party signs to accept an offer. Made by
/// [`accept_request`], completed by [`complete_accept`].
pub struct AcceptRequest {
    contract: ContractSigningRequest,
    offer: OfferDlc,
    party: PartyParams,
    payout_serial_id: u64,
    change_serial_id: u64,
    accept_collateral: Amount,
    transactions: DlcTransactions,
}

impl AcceptRequest {
    /// The refund and CETs the accepting party's contract funding key signs.
    pub fn contract(&self) -> &ContractSigningRequest {
        &self.contract
    }
}

/// What the offering party signs to answer an accept message: the refund and
/// the CETs, and its funding inputs. Made by [`sign_request`], completed by
/// [`complete_sign`].
pub struct SignRequest {
    contract: ContractSigningRequest,
    funding: FundingSigningRequest,
    offer: OfferDlc,
    accept: AcceptDlc,
    transactions: DlcTransactions,
}

impl SignRequest {
    /// The refund and CETs the offering party's contract funding key signs.
    pub fn contract(&self) -> &ContractSigningRequest {
        &self.contract
    }

    /// The offering party's funding inputs: its wallet inputs and its half of
    /// each splice input.
    pub fn funding(&self) -> &FundingSigningRequest {
        &self.funding
    }
}

/// What the accepting party signs to complete the funding transaction once the
/// sign message is verified. Made by [`finalize_request`], completed by
/// [`complete_finalize`].
pub struct FinalizeRequest {
    funding: FundingSigningRequest,
    offer: OfferDlc,
    accept: AcceptDlc,
    sign: SignDlc,
    funding_transaction: Transaction,
}

impl FinalizeRequest {
    /// The accepting party's funding inputs: its wallet inputs and its half of
    /// each splice input.
    pub fn funding(&self) -> &FundingSigningRequest {
        &self.funding
    }
}

/// Validates an offer and says what the accepting party signs to accept it.
///
/// Takes the same parameters as [`accept_offer`](super::accept_offer), less the
/// funding secret key; `params.party.funding_pubkey` is the key the signer must
/// sign with. Serial ids omitted from `params` are drawn here, so a request
/// made again from the same parameters is a different request.
pub fn accept_request(
    offer: &OfferDlc,
    params: AcceptOfferParams,
) -> Result<AcceptRequest, ContractError> {
    let AcceptOfferParams {
        party,
        min_timeout_interval,
        max_timeout_interval,
        now_unix,
    } = params;

    validate_offer(offer, min_timeout_interval, max_timeout_interval, now_unix)?;
    ensure_no_dlc_inputs(&party.funding_inputs)?;
    ensure_unique_input_serial_ids(offer, &party.funding_inputs)?;
    let accept_collateral = offer
        .get_total_collateral()
        .checked_sub(offer.offer_collateral)
        .ok_or_else(|| {
            ContractError::InvalidOffer("offer collateral exceeds total collateral".to_string())
        })?;

    let payout_serial_id = party.payout_serial_id.unwrap_or_else(random_serial_id);
    let change_serial_id = party.change_serial_id.unwrap_or_else(random_serial_id);
    let accept_params = dlc_party_params(
        party.funding_pubkey,
        party.payout_spk.clone(),
        payout_serial_id,
        party.change_spk.clone(),
        change_serial_id,
        accept_collateral,
        &party.funding_inputs,
    )?;
    let context = build_context(offer, &accept_params)?;
    let adaptor_points =
        cet_adaptor_points(&context.execution_infos, offer.get_total_collateral())?;
    Ok(AcceptRequest {
        contract: ContractSigningRequest::new(
            &context.transactions,
            adaptor_points,
            party.funding_pubkey,
        )?,
        offer: offer.clone(),
        party,
        payout_serial_id,
        change_serial_id,
        accept_collateral,
        transactions: context.transactions,
    })
}

/// Verifies the accepting party's signatures and builds the accept message.
pub fn complete_accept(
    request: AcceptRequest,
    signatures: ContractSignatures,
) -> Result<AcceptResult, ContractError> {
    let AcceptRequest {
        contract,
        offer,
        party,
        payout_serial_id,
        change_serial_id,
        accept_collateral,
        transactions,
    } = request;
    let refund_signature = contract.verify(&transactions, &signatures)?;
    let accept = AcceptDlc {
        protocol_version: offer.protocol_version,
        temporary_contract_id: offer.temporary_contract_id,
        accept_collateral,
        funding_pubkey: party.funding_pubkey,
        payout_spk: party.payout_spk,
        payout_serial_id,
        funding_inputs: party.funding_inputs,
        change_spk: party.change_spk,
        change_serial_id,
        cet_adaptor_signatures: CetAdaptorSignatures::from(
            signatures.cet_adaptor_signatures.as_slice(),
        ),
        refund_signature,
        negotiation_fields: None,
        tlvs: Default::default(),
    };
    let funding_psbt = build_funding_psbt(&offer, &accept, transactions.fund.clone())?;
    Ok(AcceptResult {
        accept,
        transactions,
        funding_psbt,
    })
}

/// Verifies an accept message and says what the offering party signs to
/// answer it.
pub fn sign_request(offer: &OfferDlc, accept: &AcceptDlc) -> Result<SignRequest, ContractError> {
    let context = context_from_messages(offer, accept)?;
    let adaptor_points =
        cet_adaptor_points(&context.execution_infos, offer.get_total_collateral())?;
    verify_contract_signatures(
        &Secp256k1::new(),
        &context.transactions,
        &adaptor_points,
        &accept.funding_pubkey,
        &accept.refund_signature,
        &Vec::from(&accept.cet_adaptor_signatures),
        ContractError::InvalidAccept,
    )?;
    Ok(SignRequest {
        contract: ContractSigningRequest::new(
            &context.transactions,
            adaptor_points,
            offer.funding_pubkey,
        )?,
        funding: FundingSigningRequest::new(offer, accept, &context.transactions, Party::Offer)?,
        offer: offer.clone(),
        accept: accept.clone(),
        transactions: context.transactions,
    })
}

/// Verifies the offering party's signatures and builds the sign message.
///
/// `signed_funding_psbt` is the request's funding PSBT as the signer returned
/// it (see [`FundingSigningRequest`]).
pub fn complete_sign(
    request: SignRequest,
    signatures: ContractSignatures,
    signed_funding_psbt: &Psbt,
) -> Result<SignResult, ContractError> {
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &request.transactions.fund)?;
    let funding_signatures = extract_funding_signatures(
        &request.offer,
        &request.accept,
        Party::Offer,
        signed_funding_psbt,
    )?;
    complete_sign_with_funding_signatures(request, signatures, funding_signatures)
}

/// [`complete_sign`] with the offering party's funding signatures already in
/// wire form, one per offer funding input in message order; a splice input's
/// is its half signature.
pub(crate) fn complete_sign_with_funding_signatures(
    request: SignRequest,
    signatures: ContractSignatures,
    funding_signatures: FundingSignatures,
) -> Result<SignResult, ContractError> {
    let SignRequest {
        contract,
        offer,
        accept,
        transactions,
        ..
    } = request;
    if funding_signatures.funding_signatures.len() != offer.funding_inputs.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} offer funding signatures, received {}",
            offer.funding_inputs.len(),
            funding_signatures.funding_signatures.len()
        )));
    }
    verify_offer_splice_signatures(
        &offer,
        &accept,
        &transactions.fund,
        &funding_signatures,
        ContractError::InvalidSignature,
    )?;
    let refund_signature = contract.verify(&transactions, &signatures)?;

    let sign = SignDlc {
        protocol_version: offer.protocol_version,
        contract_id: contract_id_from_transactions(&transactions, &offer.temporary_contract_id),
        cet_adaptor_signatures: CetAdaptorSignatures::from(
            signatures.cet_adaptor_signatures.as_slice(),
        ),
        refund_signature,
        funding_signatures,
        tlvs: Default::default(),
    };
    Ok(SignResult { sign, transactions })
}

/// Verifies a sign message and says what the accepting party signs to
/// complete the funding transaction.
///
/// The offering party's refund and CET adaptor signatures and its half of
/// each splice input are verified here, before anything is signed.
pub fn finalize_request(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
) -> Result<FinalizeRequest, ContractError> {
    let context = context_from_messages(offer, accept)?;
    ensure_sign_message(offer, sign, &context)?;
    let offer_signatures = &sign.funding_signatures.funding_signatures;
    if offer_signatures.len() != offer.funding_inputs.len() {
        return Err(ContractError::InvalidSign(format!(
            "sign message carries {} funding signatures but the offer has {} funding inputs",
            offer_signatures.len(),
            offer.funding_inputs.len()
        )));
    }

    verify_contract_signatures(
        &Secp256k1::new(),
        &context.transactions,
        &cet_adaptor_points(&context.execution_infos, offer.get_total_collateral())?,
        &offer.funding_pubkey,
        &sign.refund_signature,
        &Vec::from(&sign.cet_adaptor_signatures),
        ContractError::InvalidSign,
    )?;
    verify_offer_splice_signatures(
        offer,
        accept,
        &context.transactions.fund,
        &sign.funding_signatures,
        ContractError::InvalidSign,
    )?;

    Ok(FinalizeRequest {
        funding: FundingSigningRequest::new(offer, accept, &context.transactions, Party::Accept)?,
        offer: offer.clone(),
        accept: accept.clone(),
        sign: sign.clone(),
        funding_transaction: context.transactions.fund,
    })
}

/// Verifies the accepting party's funding signatures and completes the funding
/// transaction, ready to broadcast.
///
/// `signed_funding_psbt` is the request's funding PSBT as the signer returned
/// it (see [`FundingSigningRequest`]). For a contract the accepting party puts
/// no inputs into and splices nothing, the unsigned PSBT is enough.
pub fn complete_finalize(
    request: FinalizeRequest,
    signed_funding_psbt: &Psbt,
) -> Result<Transaction, ContractError> {
    ensure_psbt_matches_funding_transaction(signed_funding_psbt, &request.funding_transaction)?;
    let funding_signatures = extract_funding_signatures(
        &request.offer,
        &request.accept,
        Party::Accept,
        signed_funding_psbt,
    )?;
    complete_finalize_with_funding_signatures(request, funding_signatures, signed_funding_psbt)
}

/// [`complete_finalize`] with the accepting party's wallet witnesses already
/// in wire form, one per accept funding input in message order, and its splice
/// halves as partial signatures in `splice_signatures`, a funding PSBT already
/// verified against the funding transaction.
pub(crate) fn complete_finalize_with_funding_signatures(
    request: FinalizeRequest,
    funding_signatures: FundingSignatures,
    splice_signatures: &Psbt,
) -> Result<Transaction, ContractError> {
    let FinalizeRequest {
        offer,
        accept,
        sign,
        funding_transaction: unsigned,
        ..
    } = request;
    if funding_signatures.funding_signatures.len() != accept.funding_inputs.len() {
        return Err(ContractError::InvalidFundingInput(format!(
            "expected {} accept funding signatures, received {}",
            accept.funding_inputs.len(),
            funding_signatures.funding_signatures.len()
        )));
    }

    let mut funding_transaction = unsigned.clone();
    apply_funding_signatures(
        &mut funding_transaction,
        &offer,
        &accept,
        Party::Offer,
        &sign.funding_signatures,
    )?;
    apply_funding_signatures(
        &mut funding_transaction,
        &offer,
        &accept,
        Party::Accept,
        &funding_signatures,
    )?;

    // Both halves sign the unsigned funding transaction: a SegWit sighash does
    // not commit to the other inputs' witnesses.
    let secp = Secp256k1::new();
    for (input, offer_signature) in offer
        .funding_inputs
        .iter()
        .zip(&sign.funding_signatures.funding_signatures)
    {
        let Some(dlc_input) = &input.dlc_input else {
            continue;
        };
        let input_index = funding_input_index(&offer, &accept, input.input_serial_id)?;
        let accept_half = splice_signature(
            splice_signatures,
            input_index,
            &dlc_input.remote_fund_pubkey,
        )?;
        verify_splice_signature(
            &secp,
            &unsigned,
            input_index,
            input,
            &accept_half,
            &dlc_input.remote_fund_pubkey,
        )
        .map_err(ContractError::InvalidSignature)?;
        // `finalize_request` verified the offering party's half.
        funding_transaction.input[input_index].witness =
            ddk_dlc::dlc_input::combine_dlc_input_signatures(
                &input.into(),
                &accept_half,
                &offer_signature.witness_elements[0].witness,
                &dlc_input.remote_fund_pubkey,
                &dlc_input.local_fund_pubkey,
            );
    }
    Ok(funding_transaction)
}

/// Verifies the offering party's half of each splice input, as the sign
/// message carries it: the first witness element of the input's funding
/// signature.
///
/// `error` attributes a failure to the sign message received, or to this
/// party's own signer when it is building one.
fn verify_offer_splice_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_transaction: &Transaction,
    offer_signatures: &FundingSignatures,
    error: fn(String) -> ContractError,
) -> Result<(), ContractError> {
    let secp = Secp256k1::new();
    for (input, signature) in offer
        .funding_inputs
        .iter()
        .zip(&offer_signatures.funding_signatures)
    {
        let Some(dlc_input) = &input.dlc_input else {
            continue;
        };
        let input_index = funding_input_index(offer, accept, input.input_serial_id)?;
        let half = signature
            .witness_elements
            .first()
            .map(|element| element.witness.as_slice())
            .unwrap_or_default();
        verify_splice_signature(
            &secp,
            funding_transaction,
            input_index,
            input,
            half,
            &dlc_input.local_fund_pubkey,
        )
        .map_err(error)?;
    }
    Ok(())
}

impl ContractSigningRequest {
    pub(crate) fn new(
        transactions: &DlcTransactions,
        adaptor_points: Vec<(usize, PublicKey)>,
        funding_pubkey: PublicKey,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            funding_pubkey,
            refund: funding_spend_psbt(transactions, &transactions.refund)?,
            cets: transactions
                .cets
                .iter()
                .map(|cet| funding_spend_psbt(transactions, cet))
                .collect::<Result<_, _>>()?,
            adaptor_points,
        })
    }

    /// Verifies a signer's answer against the transactions this request was
    /// made from, and returns the refund signature in the form the messages
    /// carry.
    fn verify(
        &self,
        transactions: &DlcTransactions,
        signatures: &ContractSignatures,
    ) -> Result<Signature, ContractError> {
        let refund = signatures.refund_signature;
        if refund.sighash_type != EcdsaSighashType::All {
            return Err(ContractError::InvalidSignature(format!(
                "the refund is signed with {}, SIGHASH_ALL is required",
                refund.sighash_type
            )));
        }
        verify_contract_signatures(
            &Secp256k1::new(),
            transactions,
            &self.adaptor_points,
            &self.funding_pubkey,
            &refund.signature,
            &signatures.cet_adaptor_signatures,
            ContractError::InvalidSignature,
        )?;
        Ok(refund.signature)
    }
}

impl FundingSigningRequest {
    fn new(
        offer: &OfferDlc,
        accept: &AcceptDlc,
        transactions: &DlcTransactions,
        party: Party,
    ) -> Result<Self, ContractError> {
        // Only the offering party contributes splice inputs, and both parties
        // sign half of each: the offering party with the input's local key,
        // the accepting party with its remote key.
        let splice_inputs = offer
            .funding_inputs
            .iter()
            .filter_map(|input| input.dlc_input.as_ref().map(|dlc_input| (input, dlc_input)))
            .map(|(input, dlc_input)| {
                Ok(SpliceInputToSign {
                    input_serial_id: input.input_serial_id,
                    input_index: funding_input_index(offer, accept, input.input_serial_id)?,
                    funding_pubkey: match party {
                        Party::Offer => dlc_input.local_fund_pubkey,
                        Party::Accept => dlc_input.remote_fund_pubkey,
                    },
                    temporary_contract_id: prior_temporary_contract_id(input, dlc_input)?,
                })
            })
            .collect::<Result<_, ContractError>>()?;
        Ok(Self {
            psbt: build_funding_psbt(offer, accept, transactions.fund.clone())?,
            splice_inputs,
        })
    }
}

/// A transaction spending the contract's 2-of-2 funding output, as a PSBT
/// carrying everything a signer needs to compute its sighash.
fn funding_spend_psbt(
    transactions: &DlcTransactions,
    transaction: &Transaction,
) -> Result<Psbt, ContractError> {
    let mut psbt = Psbt::from_unsigned_tx(transaction.clone())
        .map_err(|e| ContractError::PsbtMismatch(format!("could not create PSBT: {e}")))?;
    let input = &mut psbt.inputs[0];
    input.witness_utxo = Some(transactions.get_fund_output().clone());
    input.witness_script = Some(transactions.funding_witness_script.clone());
    input.sighash_type = Some(EcdsaSighashType::All.into());
    Ok(psbt)
}

// The signer in this process. The key-in-process lifecycle functions answer
// a request with these, exactly as a signer elsewhere would: from the PSBTs.

/// Answers a contract signing request with the contract funding key.
///
/// `error` attributes a key that is not the requested one to the message the
/// caller is building.
pub(crate) fn sign_contract(
    request: &ContractSigningRequest,
    funding_secret_key: &SecretKey,
    error: fn(String) -> ContractError,
) -> Result<ContractSignatures, ContractError> {
    let secp = Secp256k1::new();
    ensure_funding_key(&secp, funding_secret_key, &request.funding_pubkey, error)?;
    let (refund_sighash, sighash_type) = psbt_sighash(&request.refund, 0)?;
    let refund_signature = bitcoin::ecdsa::Signature {
        signature: secp.sign_ecdsa_low_r(&refund_sighash, funding_secret_key),
        sighash_type,
    };
    // A CET is signed once per outcome combination; its sighash is computed once.
    let cet_sighashes = request
        .cets
        .iter()
        .map(|cet| Ok(psbt_sighash(cet, 0)?.0))
        .collect::<Result<Vec<_>, ContractError>>()?;
    let cet_adaptor_signatures = request
        .adaptor_points
        .iter()
        .map(|(cet_index, adaptor_point)| {
            EcdsaAdaptorSignature::encrypt(
                &secp,
                &cet_sighashes[*cet_index],
                funding_secret_key,
                adaptor_point,
            )
        })
        .collect();
    Ok(ContractSignatures {
        refund_signature,
        cet_adaptor_signatures,
    })
}

/// Signs this party's half of each splice input `keys` has a key for, as a
/// partial signature in `psbt`. A splice input without a key is left as it
/// is, for a half that is already there; completion rejects a missing one.
pub(crate) fn sign_splice_inputs(
    request: &FundingSigningRequest,
    keys: &[DlcInputSigningKey],
    psbt: &mut Psbt,
) -> Result<(), ContractError> {
    let secp = Secp256k1::new();
    for splice in &request.splice_inputs {
        let Some(key) = keys
            .iter()
            .find(|key| key.input_serial_id == splice.input_serial_id)
        else {
            continue;
        };
        if PublicKey::from_secret_key(&secp, &key.prior_funding_secret_key) != splice.funding_pubkey
        {
            return Err(ContractError::InvalidFundingInput(format!(
                "prior funding secret key for DLC input serial id {} does not match the funding \
                 public key {} this party signs it with",
                splice.input_serial_id, splice.funding_pubkey
            )));
        }
        let (sighash, sighash_type) = psbt_sighash(&request.psbt, splice.input_index)?;
        let signature = bitcoin::ecdsa::Signature {
            signature: secp.sign_ecdsa_low_r(&sighash, &key.prior_funding_secret_key),
            sighash_type,
        };
        psbt.inputs
            .get_mut(splice.input_index)
            .ok_or_else(|| {
                ContractError::PsbtMismatch(format!(
                    "PSBT input {} does not exist",
                    splice.input_index
                ))
            })?
            .partial_sigs
            .insert(bitcoin::PublicKey::new(splice.funding_pubkey), signature);
    }
    Ok(())
}

/// The sighash of a PSBT input, computed from the PSBT the way any PSBT signer
/// computes it.
fn psbt_sighash(
    psbt: &Psbt,
    input_index: usize,
) -> Result<(Message, EcdsaSighashType), ContractError> {
    psbt.sighash_ecdsa(input_index, &mut SighashCache::new(&psbt.unsigned_tx))
        .map_err(|e| {
            ContractError::PsbtMismatch(format!(
                "could not compute the sighash of PSBT input {input_index}: {e}"
            ))
        })
}

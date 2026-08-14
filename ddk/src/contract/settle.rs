//! Contract settlement: turning a funded contract into a spendable transaction.
//!
//! Settlement is the mirror image of funding. Funding combines two parties'
//! wallet signatures into the transaction that *creates* the 2-of-2 output;
//! settlement combines two parties' funding-key signatures into the transaction
//! that *spends* it. Either the oracles attest and a CET is broadcast
//! ([`sign_cet`]), or nobody does and the refund transaction is broadcast after
//! its locktime ([`sign_refund`]).
//!
//! Like the rest of the module, nothing is stored: the CET set, the refund
//! transaction, and the adaptor information are all rebuilt from the offer and
//! accept messages on demand.

use bitcoin::Transaction;
use ddk_dlc::secp256k1_zkp::{
    ecdsa::Signature, All, EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey,
};
use ddk_messages::oracle_msgs::OracleAttestation;
use ddk_messages::{AcceptDlc, OfferDlc, SignDlc};

use super::context::{context_from_messages, ensure_sign_message};
use super::error::ContractError;
use super::types::Party;

/// Signs the CET matching a set of oracle attestations.
///
/// The returned transaction spends the contract's funding output and pays each
/// party the outcome's payout. Broadcast it with the chain client of your
/// choice; this function performs no network access.
///
/// `funding_secret_key` is the settling party's DLC funding key. It is required
/// because settling means producing *this* party's half of the 2-of-2 funding
/// signature — the counterparty's half comes from decrypting its CET adaptor
/// signature with the oracle signatures. The key also identifies which side is
/// settling, so there is no party argument to get wrong: whichever of
/// `offer.funding_pubkey` and `accept.funding_pubkey` it matches determines
/// whose adaptor signatures are used.
///
/// `attestations` pairs each attestation with the index of its oracle in the
/// announcements of the contract info it settles. For a contract with several
/// disjoint contract infos, the first one whose outcome the attestations
/// resolve is used, so it is enough to pass the attestations for one event.
///
/// Returns [`ContractError::NoMatchingOutcome`] when no contract outcome
/// corresponds to the attested outcomes, which is also what an attestation for
/// an event this contract does not use looks like.
pub fn sign_cet(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_secret_key: &SecretKey,
    attestations: &[(usize, OracleAttestation)],
) -> Result<Transaction, ContractError> {
    let secp = Secp256k1::new();
    let context = context_from_messages(offer, accept)?;
    ensure_sign_message(offer, sign, &context)?;
    let party = settling_party(&secp, offer, accept, funding_secret_key)?;
    let (counterparty_pubkey, adaptor_signatures) =
        counterparty_adaptor_signatures(offer, accept, sign, party);

    let total_collateral = offer.get_total_collateral();
    let funding_witness_script = &context.transactions.funding_witness_script;
    let fund_value = context.transactions.get_fund_output().value;
    let outcomes: Vec<(usize, &Vec<String>)> = attestations
        .iter()
        .map(|(index, attestation)| (*index, &attestation.outcomes))
        .collect();

    let mut signature_index = 0;
    for (info, cet_range) in context.execution_infos.iter().zip(&context.cet_ranges) {
        // Verifying the counterparty's adaptor signatures is also how the
        // adaptor info and the next signature offset are obtained.
        let (adaptor_info, next_index) = info
            .verify_and_get_adaptor_info(
                &secp,
                total_collateral,
                &counterparty_pubkey,
                funding_witness_script,
                fund_value,
                &context.transactions.cets[cet_range.clone()],
                &adaptor_signatures,
                signature_index,
            )
            .map_err(|e| {
                counterparty_error(party)(format!("invalid CET adaptor signatures: {e}"))
            })?;

        let Some((signature_infos, range_info)) =
            info.get_range_info_for_outcome(&adaptor_info, &outcomes, signature_index)
        else {
            signature_index = next_index;
            continue;
        };

        validate_attestations(&secp, &info.oracle_announcements, attestations)?;

        // `cet_index` is relative to the CETs of this contract info; the
        // adaptor index already carries the running offset.
        let mut cet = context.transactions.cets[cet_range.start + range_info.cet_index].clone();
        let oracle_signatures: Vec<Vec<_>> = attestations
            .iter()
            .filter_map(|(index, attestation)| {
                let signature_info = signature_infos.iter().find(|info| info.0 == *index)?;
                Some(
                    attestation
                        .signatures
                        .iter()
                        .take(signature_info.1)
                        .cloned()
                        .collect(),
                )
            })
            .collect();

        ddk_dlc::sign_cet(
            &secp,
            &mut cet,
            &adaptor_signatures[range_info.adaptor_index],
            &oracle_signatures,
            funding_secret_key,
            &counterparty_pubkey,
            funding_witness_script,
            fund_value,
        )?;
        return Ok(cet);
    }

    Err(ContractError::NoMatchingOutcome)
}

/// Signs the refund transaction.
///
/// The refund returns each party its own collateral and can only be broadcast
/// once the offer's `refund_locktime` has passed; enforcing that is the chain's
/// job, not this function's.
///
/// Both parties signed the refund during the offer/accept exchange, so this
/// only adds `funding_secret_key`'s half. As in [`sign_cet`], the key
/// identifies the settling party. The counterparty's stored signature is
/// verified before the two are combined.
pub fn sign_refund(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_secret_key: &SecretKey,
) -> Result<Transaction, ContractError> {
    let secp = Secp256k1::new();
    let context = context_from_messages(offer, accept)?;
    ensure_sign_message(offer, sign, &context)?;
    let party = settling_party(&secp, offer, accept, funding_secret_key)?;
    let (counterparty_pubkey, counterparty_signature): (PublicKey, Signature) = match party {
        Party::Offer => (accept.funding_pubkey, accept.refund_signature),
        Party::Accept => (offer.funding_pubkey, sign.refund_signature),
    };

    let funding_witness_script = &context.transactions.funding_witness_script;
    let fund_value = context.transactions.get_fund_output().value;
    ddk_dlc::verify_tx_input_sig(
        &secp,
        &counterparty_signature,
        &context.transactions.refund,
        0,
        funding_witness_script,
        fund_value,
        &counterparty_pubkey,
    )
    .map_err(|e| counterparty_error(party)(format!("invalid refund signature: {e}")))?;

    let mut refund = context.transactions.refund.clone();
    ddk_dlc::util::sign_multi_sig_input(
        &secp,
        &mut refund,
        &counterparty_signature,
        &counterparty_pubkey,
        funding_secret_key,
        funding_witness_script,
        fund_value,
        0,
    )?;
    Ok(refund)
}

/// Identifies which side of the contract a funding secret key settles for.
fn settling_party(
    secp: &Secp256k1<All>,
    offer: &OfferDlc,
    accept: &AcceptDlc,
    funding_secret_key: &SecretKey,
) -> Result<Party, ContractError> {
    let public_key = PublicKey::from_secret_key(secp, funding_secret_key);
    if public_key == offer.funding_pubkey {
        Ok(Party::Offer)
    } else if public_key == accept.funding_pubkey {
        Ok(Party::Accept)
    } else {
        Err(ContractError::Key(
            "funding secret key does not match either party's funding public key".to_string(),
        ))
    }
}

/// The counterparty's funding public key and CET adaptor signatures: the accept
/// message carries the accepting party's, the sign message the offering party's.
fn counterparty_adaptor_signatures(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    party: Party,
) -> (PublicKey, Vec<EcdsaAdaptorSignature>) {
    match party {
        Party::Offer => (
            accept.funding_pubkey,
            (&accept.cet_adaptor_signatures).into(),
        ),
        Party::Accept => (offer.funding_pubkey, (&sign.cet_adaptor_signatures).into()),
    }
}

/// Attributes a signature failure to the message the counterparty's signatures
/// arrived in.
fn counterparty_error(party: Party) -> fn(String) -> ContractError {
    match party {
        Party::Offer => ContractError::InvalidAccept,
        Party::Accept => ContractError::InvalidSign,
    }
}

/// Checks each attestation against the announcement of the oracle it claims to
/// come from, so a forged or misindexed attestation cannot produce a CET.
fn validate_attestations(
    secp: &Secp256k1<All>,
    announcements: &[ddk_messages::oracle_msgs::OracleAnnouncement],
    attestations: &[(usize, OracleAttestation)],
) -> Result<(), ContractError> {
    for (index, attestation) in attestations {
        let announcement = announcements.get(*index).ok_or_else(|| {
            ContractError::InvalidAttestation(format!(
                "attestation refers to oracle {index} but the contract has {} oracles",
                announcements.len()
            ))
        })?;
        attestation.validate(secp, announcement).map_err(|e| {
            ContractError::InvalidAttestation(format!("attestation from oracle {index}: {e}"))
        })?;
    }
    Ok(())
}

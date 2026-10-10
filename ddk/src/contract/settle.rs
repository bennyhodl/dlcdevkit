//! Contract settlement: turning a funded contract into a spendable transaction.
//!
//! Settlement is the mirror image of funding. Funding combines two parties'
//! wallet signatures into the transaction that *creates* the 2-of-2 output;
//! settlement combines two parties' funding-key signatures into the transaction
//! that *spends* it. Either the oracles attest and a CET is broadcast, or
//! nobody does and the refund transaction is broadcast after its locktime.
//!
//! Both halves of either spend are already committed in the messages: each
//! party's CET adaptor signatures and refund signature travel in the message
//! it sent (the accepting party's in the accept message, the offering party's
//! in the sign message). So there are two ways to settle, which differ only in
//! where this party's half comes from:
//!
//! | Outcome | With a funding key | From the messages alone |
//! |---------|--------------------|-------------------------|
//! | the oracles attest | [`sign_cet`] | [`settle_cet_from_messages`] |
//! | nobody attests | [`sign_refund`] | [`settle_refund_from_messages`] |
//!
//! The keyless variants let a watchtower, a server, or either party without
//! access to its key close the contract. Every path selects the CET, validates
//! the attestations, and verifies the signatures it takes from the messages in
//! the same way, so the keyless and key-based transactions are the same
//! transaction: their txids are equal, and only the witness signatures differ.
//!
//! Like the rest of the module, nothing is stored: the CET set, the refund
//! transaction, and the adaptor information are all rebuilt from the offer and
//! accept messages on demand.

use bitcoin::sighash::EcdsaSighashType;
use bitcoin::{Transaction, Witness};
use ddk_dlc::secp256k1_zkp::{
    ecdsa::Signature, All, EcdsaAdaptorSignature, PublicKey, Secp256k1, SecretKey,
};
use ddk_messages::oracle_msgs::OracleAttestation;
use ddk_messages::{AcceptDlc, CetAdaptorSignatures, OfferDlc, SignDlc};

use super::context::{signed_context, ContractContext};
use super::error::ContractError;
use super::types::Party;

/// Signs the CET matching a set of oracle attestations.
///
/// The returned transaction spends the contract's funding output and pays each
/// party the outcome's payout. Broadcast it with the chain client of your
/// choice; this function performs no network access.
///
/// `funding_secret_key` is the settling party's DLC funding key. It produces
/// *this* party's half of the 2-of-2 funding signature — the counterparty's
/// half comes from decrypting its CET adaptor signature with the oracle
/// signatures. The key also identifies which side is settling, so there is no
/// party argument to get wrong: whichever of `offer.funding_pubkey` and
/// `accept.funding_pubkey` it matches determines whose adaptor signatures are
/// used. [`settle_cet_from_messages`] builds the same transaction without the
/// key.
///
/// `attestations` pairs each attestation with the index of its oracle in the
/// announcements of the contract info it settles. For a contract with several
/// disjoint contract infos, the first one whose outcome the attestations
/// resolve is used, so it is enough to pass the attestations for one event.
/// Each oracle may appear at most once: the adaptor secret is the sum of one
/// signature set per oracle, so a repeated index is rejected with
/// [`ContractError::InvalidAttestation`] instead of being summed twice.
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
    let settlement = Settlement::new(offer, accept, sign)?;
    let counterparty = settlement
        .party_of(&secp, funding_secret_key)?
        .counterparty();
    let cet = settlement.attested_cet(&secp, counterparty, attestations)?;
    let theirs = settlement.decrypt_cet_signature(&secp, &cet, counterparty)?;
    let ours = settlement.sign_with(&secp, &cet.transaction, funding_secret_key)?;
    Ok(settlement.complete(cet.transaction, ours, theirs))
}

/// Builds the CET matching a set of oracle attestations from the messages
/// alone, with no funding key.
///
/// Both parties encrypted their CET adaptor signatures to the same adaptor
/// point, so the oracle signatures that decrypt one decrypt the other. This
/// decrypts both, verifies each against the CET under its party's funding key,
/// and places them in the witness in the funding script's key order. Anyone
/// holding the three messages and the attestations can therefore close the
/// contract: either party without its key, a watchtower, or a server.
///
/// The CET is selected and the attestations are validated exactly as in
/// [`sign_cet`], which documents `attestations` and the errors. The result is
/// the transaction [`sign_cet`] returns for either party, with the same txid.
/// A decrypted signature that does not verify is reported against the message
/// that carried it: [`ContractError::InvalidAccept`] for the accepting party's,
/// [`ContractError::InvalidSign`] for the offering party's.
pub fn settle_cet_from_messages(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    attestations: &[(usize, OracleAttestation)],
) -> Result<Transaction, ContractError> {
    let secp = Secp256k1::new();
    let settlement = Settlement::new(offer, accept, sign)?;
    // Selecting the CET verifies one party's adaptor signatures in full, since
    // that is how the adaptor information is rebuilt; either party's would do.
    // Both decrypted signatures are then verified against the selected CET.
    let cet = settlement.attested_cet(&secp, Party::Accept, attestations)?;
    let offer_half = settlement.decrypt_cet_signature(&secp, &cet, Party::Offer)?;
    let accept_half = settlement.decrypt_cet_signature(&secp, &cet, Party::Accept)?;
    Ok(settlement.complete(cet.transaction, offer_half, accept_half))
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
/// verified before the two are combined. [`settle_refund_from_messages`]
/// builds the same transaction without the key.
pub fn sign_refund(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
    funding_secret_key: &SecretKey,
) -> Result<Transaction, ContractError> {
    let secp = Secp256k1::new();
    let settlement = Settlement::new(offer, accept, sign)?;
    let counterparty = settlement
        .party_of(&secp, funding_secret_key)?
        .counterparty();
    let refund = settlement.context.transactions.refund.clone();
    let theirs = settlement.refund_signature(&secp, counterparty)?;
    let ours = settlement.sign_with(&secp, &refund, funding_secret_key)?;
    Ok(settlement.complete(refund, ours, theirs))
}

/// Builds the refund transaction from the messages alone, with no funding key.
///
/// The accept message carries the accepting party's refund signature and the
/// sign message the offering party's. Both are verified against the refund
/// transaction before they are combined, and a bad one is reported against the
/// message that carried it. The result is the transaction [`sign_refund`]
/// returns for either party, with the same txid. As there, the chain enforces
/// the refund locktime.
pub fn settle_refund_from_messages(
    offer: &OfferDlc,
    accept: &AcceptDlc,
    sign: &SignDlc,
) -> Result<Transaction, ContractError> {
    let secp = Secp256k1::new();
    let settlement = Settlement::new(offer, accept, sign)?;
    let refund = settlement.context.transactions.refund.clone();
    let offer_half = settlement.refund_signature(&secp, Party::Offer)?;
    let accept_half = settlement.refund_signature(&secp, Party::Accept)?;
    Ok(settlement.complete(refund, offer_half, accept_half))
}

/// One half of the 2-of-2 funding spend: a funding public key and its
/// signature over the settlement transaction.
type SpendHalf = (PublicKey, Signature);

/// A signed contract rebuilt from its messages, with the settlement steps every
/// public function in this module composes.
struct Settlement<'a> {
    offer: &'a OfferDlc,
    accept: &'a AcceptDlc,
    sign: &'a SignDlc,
    context: ContractContext,
}

/// What one party committed to in the message it sent.
struct Committed<'a> {
    funding_pubkey: PublicKey,
    refund_signature: Signature,
    adaptor_signatures: &'a CetAdaptorSignatures,
    /// Attributes a bad signature to the message that carried it.
    error: fn(String) -> ContractError,
}

/// The CET an attestation set selects, and what decrypts each party's adaptor
/// signature for it.
struct AttestedCet {
    transaction: Transaction,
    adaptor_index: usize,
    adaptor_secret: SecretKey,
}

impl<'a> Settlement<'a> {
    fn new(
        offer: &'a OfferDlc,
        accept: &'a AcceptDlc,
        sign: &'a SignDlc,
    ) -> Result<Self, ContractError> {
        Ok(Self {
            offer,
            accept,
            sign,
            context: signed_context(offer, accept, sign)?,
        })
    }

    /// The accept message carries the accepting party's signatures, the sign
    /// message the offering party's.
    fn committed(&self, party: Party) -> Committed<'a> {
        match party {
            Party::Offer => Committed {
                funding_pubkey: self.offer.funding_pubkey,
                refund_signature: self.sign.refund_signature,
                adaptor_signatures: &self.sign.cet_adaptor_signatures,
                error: ContractError::InvalidSign,
            },
            Party::Accept => Committed {
                funding_pubkey: self.accept.funding_pubkey,
                refund_signature: self.accept.refund_signature,
                adaptor_signatures: &self.accept.cet_adaptor_signatures,
                error: ContractError::InvalidAccept,
            },
        }
    }

    /// Identifies which side of the contract a funding secret key settles for.
    fn party_of(
        &self,
        secp: &Secp256k1<All>,
        funding_secret_key: &SecretKey,
    ) -> Result<Party, ContractError> {
        let public_key = PublicKey::from_secret_key(secp, funding_secret_key);
        if public_key == self.offer.funding_pubkey {
            Ok(Party::Offer)
        } else if public_key == self.accept.funding_pubkey {
            Ok(Party::Accept)
        } else {
            Err(ContractError::Key(
                "funding secret key does not match either party's funding public key".to_string(),
            ))
        }
    }

    /// Finds the CET the attestations select and the secret that decrypts its
    /// adaptor signatures.
    ///
    /// Rebuilding the adaptor information means verifying a full set of
    /// adaptor signatures, so `selector` names the party whose set is used.
    fn attested_cet(
        &self,
        secp: &Secp256k1<All>,
        selector: Party,
        attestations: &[(usize, OracleAttestation)],
    ) -> Result<AttestedCet, ContractError> {
        let selector = self.committed(selector);
        let adaptor_signatures: Vec<EcdsaAdaptorSignature> = selector.adaptor_signatures.into();
        let transactions = &self.context.transactions;
        let total_collateral = self.offer.get_total_collateral();
        let fund_value = transactions.get_fund_output().value;

        let mut signature_index = 0;
        for (info, cet_range) in self
            .context
            .execution_infos
            .iter()
            .zip(&self.context.cet_ranges)
        {
            let (adaptor_info, next_index) = info
                .verify_and_get_adaptor_info(
                    secp,
                    total_collateral,
                    &selector.funding_pubkey,
                    &transactions.funding_witness_script,
                    fund_value,
                    &transactions.cets[cet_range.clone()],
                    &adaptor_signatures,
                    signature_index,
                )
                .map_err(|e| (selector.error)(format!("invalid CET adaptor signatures: {e}")))?;

            // The lookup also binds the attestations to the oracle combination
            // the adaptor point was built for: one attestation per oracle, and
            // a signature set for every oracle of the matched combination.
            let Some((range_info, oracle_signatures)) = info
                .get_range_info_and_oracle_signatures(&adaptor_info, attestations, signature_index)
                .map_err(attestation_error)?
            else {
                signature_index = next_index;
                continue;
            };

            validate_attestations(secp, &info.oracle_announcements, attestations)?;
            let adaptor_secret = ddk_dlc::adaptor_secret(&oracle_signatures)
                .map_err(|e| ContractError::InvalidAttestation(e.to_string()))?;

            // `cet_index` is relative to the CETs of this contract info; the
            // adaptor index already carries the running offset.
            return Ok(AttestedCet {
                transaction: transactions.cets[cet_range.start + range_info.cet_index].clone(),
                adaptor_index: range_info.adaptor_index,
                adaptor_secret,
            });
        }

        Err(ContractError::NoMatchingOutcome)
    }

    /// Decrypts `party`'s adaptor signature for the attested CET.
    fn decrypt_cet_signature(
        &self,
        secp: &Secp256k1<All>,
        cet: &AttestedCet,
        party: Party,
    ) -> Result<SpendHalf, ContractError> {
        let committed = self.committed(party);
        let adaptor_signature = committed
            .adaptor_signatures
            .ecdsa_adaptor_signatures
            .get(cet.adaptor_index)
            .ok_or_else(|| {
                (committed.error)(format!(
                    "no CET adaptor signature at index {}",
                    cet.adaptor_index
                ))
            })?;
        let signature = adaptor_signature
            .signature
            .decrypt(&cet.adaptor_secret)
            .map_err(|e| (committed.error)(format!("invalid CET adaptor signature: {e}")))?;
        self.verified(secp, &cet.transaction, &committed, signature, "CET")
    }

    /// `party`'s refund signature, from the message it sent.
    fn refund_signature(
        &self,
        secp: &Secp256k1<All>,
        party: Party,
    ) -> Result<SpendHalf, ContractError> {
        let committed = self.committed(party);
        let refund = &self.context.transactions.refund;
        self.verified(
            secp,
            refund,
            &committed,
            committed.refund_signature,
            "refund",
        )
    }

    /// Checks that a signature taken from the messages spends the funding
    /// output in `transaction`, so a bad one is attributed to its message
    /// instead of surfacing as an invalid transaction at broadcast.
    fn verified(
        &self,
        secp: &Secp256k1<All>,
        transaction: &Transaction,
        committed: &Committed,
        signature: Signature,
        what: &str,
    ) -> Result<SpendHalf, ContractError> {
        let transactions = &self.context.transactions;
        ddk_dlc::verify_tx_input_sig(
            secp,
            &signature,
            transaction,
            0,
            &transactions.funding_witness_script,
            transactions.get_fund_output().value,
            &committed.funding_pubkey,
        )
        .map_err(|e| (committed.error)(format!("invalid {what} signature: {e}")))?;
        Ok((committed.funding_pubkey, signature))
    }

    /// Produces this party's half of the funding spend with its funding key.
    fn sign_with(
        &self,
        secp: &Secp256k1<All>,
        transaction: &Transaction,
        funding_secret_key: &SecretKey,
    ) -> Result<SpendHalf, ContractError> {
        let transactions = &self.context.transactions;
        let signature = ddk_dlc::util::get_raw_sig_for_tx_input(
            secp,
            transaction,
            0,
            &transactions.funding_witness_script,
            transactions.get_fund_output().value,
            funding_secret_key,
        )?;
        Ok((
            PublicKey::from_secret_key(secp, funding_secret_key),
            signature,
        ))
    }

    /// Places both halves on the funding input's witness, in the key order of
    /// the funding script that `OP_CHECKMULTISIG` checks them against. On a
    /// key tie `b` goes first, as in `ddk_dlc::util::sign_multi_sig_input`,
    /// so a key-based close keeps the witness bytes it always had.
    fn complete(&self, mut transaction: Transaction, a: SpendHalf, b: SpendHalf) -> Transaction {
        let (first, second) = if a.0 < b.0 { (a, b) } else { (b, a) };
        transaction.input[0].witness = Witness::from_slice(&[
            Vec::new(),
            ddk_dlc::util::finalize_sig(&first.1, EcdsaSighashType::All),
            ddk_dlc::util::finalize_sig(&second.1, EcdsaSighashType::All),
            self.context.transactions.funding_witness_script.to_bytes(),
        ]);
        transaction
    }
}

/// Reports an attestation set that does not bind to the contract's oracles.
fn attestation_error(error: ddk_manager::error::Error) -> ContractError {
    match error {
        ddk_manager::error::Error::InvalidParameters(message) => {
            ContractError::InvalidAttestation(message)
        }
        other => ContractError::InvalidAttestation(other.to_string()),
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
